// IDAPython evaluation for the UI's console and script runner. A small runner
// is installed once into __main__; each request hands it the source base64
// encoded (never spliced into Python text), and it returns the output as
// ordered (stream, text) chunks plus the traceback as data. Streams are told
// apart by swapping sys.stdout / sys.stderr for tagging writers, since IDA's
// message hook sees one undifferentiated text stream.

#include <pro.h>
#include <ida.hpp>
#include <expr.hpp>
#include <kernwin.hpp>
#include <idp.hpp>

#include <atomic>
#include <chrono>

#include "common.hpp"
#include "events.hpp"

extern "C" unsigned long izs_thread_id(void);
extern "C" int izs_py_interrupt(unsigned long tid);

static const char RUNNER[] = R"PY(
import sys as _s, ast as _ast, codeop as _codeop, traceback as _tb, json as _json
import base64 as _b64, linecache as _lc, threading as _th
import idaapi, idc, idautils

class _IzsW:
    def __init__(self, tag, chunks, orig):
        self.tag = tag
        self.chunks = chunks
        self.orig = orig
    def write(self, t):
        if not isinstance(t, str):
            t = str(t)
        if t:
            ch = self.chunks
            if ch and ch[-1][0] == self.tag:
                ch[-1][1] += t
            else:
                ch.append([self.tag, t])
            try:
                self.orig.write(t)
            except Exception:
                pass
        return len(t)
    def flush(self):
        try:
            self.orig.flush()
        except Exception:
            pass
    def isatty(self):
        return False

def _izs_src(b64):
    return _b64.b64decode(b64).decode('utf-8', 'replace').replace('\r\n', '\n')

def _izs_complete(b64):
    # A last line holding only the auto-indent counts as the blank line that
    # ends a block.
    src = _izs_src(b64).rstrip(' 	')
    try:
        return '0' if _codeop.compile_command(src, '<console>', 'single') is None else '1'
    except (SyntaxError, ValueError, OverflowError):
        pass
    try:
        return '0' if _codeop.compile_command(src, '<console>', 'exec') is None else '1'
    except (SyntaxError, ValueError, OverflowError):
        return '1'

def _izs_run(b64, mode, path):
    g = _s.modules['__main__'].__dict__
    chunks = []
    error = None
    old = (_s.stdout, _s.stderr)
    _s.stdout = _IzsW('out', chunks, old[0])
    _s.stderr = _IzsW('err', chunks, old[1])
    try:
        try:
            if mode == 2:
                with open(path, 'rb') as f:
                    data = f.read()
                code = compile(data, path, 'exec')
                had = '__file__' in g
                prev = g.get('__file__')
                g['__file__'] = path
                try:
                    exec(code, g)
                finally:
                    if had:
                        g['__file__'] = prev
                    else:
                        g.pop('__file__', None)
            else:
                src = _izs_src(b64)
                lines = src.splitlines(True)
                _lc.cache['<console>'] = (len(src), None, lines, '<console>')
                tree = _ast.parse(src, '<console>', 'exec')
                last = None
                if tree.body and isinstance(tree.body[-1], _ast.Expr):
                    last = tree.body.pop()
                if tree.body:
                    exec(compile(tree, '<console>', 'exec'), g)
                if last is not None:
                    val = eval(compile(_ast.Expression(last.value), '<console>', 'eval'), g)
                    if val is not None:
                        g['_'] = val
                        chunks.append(['result', repr(val) + '\n'])
        except BaseException:
            et, ev, tb = _s.exc_info()
            here = _izs_run.__code__.co_filename
            while tb is not None and tb.tb_frame.f_code.co_filename == here:
                tb = tb.tb_next
            error = ''.join(_tb.format_exception(et, ev, tb)).rstrip('\n')
            chunks.append(['tb', error + '\n'])
    finally:
        _s.stdout, _s.stderr = old
    return _json.dumps({'chunks': chunks, 'error': error})
)PY";

static std::string base64(const char *s)
{
  static const char tbl[] = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
  const unsigned char *p = (const unsigned char *)s;
  size_t n = strlen(s);
  std::string out;
  out.reserve((n + 2) / 3 * 4);
  for ( size_t i = 0; i < n; i += 3 )
  {
    uint32_t v = p[i] << 16;
    if ( i + 1 < n )
      v |= p[i + 1] << 8;
    if ( i + 2 < n )
      v |= p[i + 2];
    out += tbl[(v >> 18) & 63];
    out += tbl[(v >> 12) & 63];
    out += i + 1 < n ? tbl[(v >> 6) & 63] : '=';
    out += i + 2 < n ? tbl[v & 63] : '=';
  }
  return out;
}

static std::string py_quote(const std::string &s)
{
  std::string out = "'";
  for ( char c : s )
  {
    if ( c == '\\' || c == '\'' )
      out += '\\';
    out += c;
  }
  return out + "'";
}

static bool g_runner = false;

// Runs `code`, then reads the string variable `var`.
static bool py_call(extlang_t *py, const std::string &code, const char *var, qstring *result, qstring *err)
{
  if ( !py->eval_snippet(code.c_str(), err) )
    return false;
  idc_value_t rv;
  qstring e2;
  if ( !py->eval_expr(&rv, BADADDR, var, &e2) )
  {
    *err = e2;
    return false;
  }
  if ( rv.vtype != VT_STR )
  {
    *err = "the Python runner returned no text";
    return false;
  }
  *result = rv.qstr();
  return true;
}

// The thread running Python while an evaluation is in flight, else 0.
static std::atomic<unsigned long> g_py_tid{0};

struct TidScope
{
  TidScope() { g_py_tid = izs_thread_id(); }
  ~TidScope() { g_py_tid = 0; }
};

extern "C" {

// mode: 0 = console, run only a complete statement; 1 = console, run what
// there is; 2 = run the script file `path`.
int izs_py_eval(const char *source, uint64_t ea, int mode, const char *path)
{
  REQUIRE_DB();
  GUARD_BEGIN
  extlang_object_t py = find_extlang_by_name("Python");
  if ( py == nullptr )
    return fail("the Python extension language is not available");
  qstring err;
  ev_python_begin();
  if ( !g_runner )
  {
    if ( !py->eval_snippet(RUNNER, &err) )
      return fail("cannot install the Python runner: %s", err.c_str());
    g_runner = true;
  }
  std::string b64 = base64(source);
  if ( mode == 0 )
  {
    qstring done;
    if ( !py_call(py, "_izs_out = _izs_complete('" + b64 + "')", "_izs_out", &done, &err) )
      return fail("%s", err.c_str());
    if ( done[0] == '0' )
    {
      Json j;
      j.obj().kvb("incomplete", true).key("chunks").arr().end_arr().key("error").null();
      j.kvb("changed", false).kv("elapsed_ms", (uint64_t)0).end_obj();
      return ok(j);
    }
  }
  undo_point();
  ev_set_screen_ea((ea_t)ea);
  auto t0 = std::chrono::steady_clock::now();
  qstring out;
  TidScope scope;
  std::string code = "_izs_out = _izs_run('" + b64 + "', " + (mode == 2 ? "2" : "0") + ", " +
                     py_quote(path != nullptr ? path : "") + ")";
  if ( !py_call(py, code, "_izs_out", &out, &err) )
    return fail("%s", err.c_str());
  long long ms = (long long)std::chrono::duration_cast<std::chrono::milliseconds>(
                   std::chrono::steady_clock::now() - t0).count();
  // The runner's JSON object is ASCII; extend it with the host's facts.
  std::string s(out.c_str(), out.length());
  if ( s.empty() || s.back() != '}' )
    return fail("the Python runner returned malformed data");
  s.pop_back();
  s += ",\"incomplete\":false,\"changed\":";
  s += ev_python_changed() ? "true" : "false";
  s += ",\"elapsed_ms\":" + std::to_string(ms) + "}";
  g_result = std::move(s);
  return 0;
  GUARD_END
}

// Raises KeyboardInterrupt in the thread running Python. Safe to call from
// any thread while izs_py_eval runs.
int izs_interrupt(void)
{
  unsigned long tid = g_py_tid.load();
  return tid != 0 ? izs_py_interrupt(tid) : 1;
}

}
