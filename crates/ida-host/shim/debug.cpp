// Local debugger (win32 user mode) through the kernel's own debugger API.
//
// idalib has no UI loop, so the host drives the kernel: requests start,
// resume or step the process and return at once; `izs_dbg_poll` (called by
// the host loop while a process runs) waits for debugger events in short
// slices and reports each one as a `{"event":"debug","kind":...}` line.
//
// Facts this file relies on (docs/IDALIB_FINDINGS.md):
//   * at process start the kernel rebases the database to the runtime image
//     base and leaves it there after the process exits. A rebased database
//     must never be saved, so the base recorded before the session is
//     restored when the process is gone;
//   * `read_dbg_memory` shows 0xCC at software breakpoints;
//   * continuing from a breakpoint delivers a STEP event while the process
//     still runs, so a stop is decided from the process state, not the event.

#include <pro.h>
#include <ida.hpp>
#include <idp.hpp>
#include <kernwin.hpp>
#include <dbg.hpp>
#include <bytes.hpp>
#include <expr.hpp>
#include <funcs.hpp>
#include <name.hpp>
#include <nalt.hpp>
#include <segment.hpp>
#include <ua.hpp>

#include <chrono>
#include <map>
#include <string>
#include <vector>

#include "common.hpp"
#include "debug.hpp"
#include "events.hpp"

using Clock = std::chrono::steady_clock;

namespace
{
bool g_loaded = false;
bool g_alive = false;       // a debuggee exists
ea_t g_orig_base = BADADDR; // image base of the database before the session
ea_t g_last_base = BADADDR; // image base last reported to the UI
int g_reported = DSTATE_NOTASK;
int g_pid = 0;
bool g_pausing = false;
Clock::time_point g_pause_deadline;
std::map<ea_t, int> g_hits; // hit counts of stops at user breakpoints
// The previous event when it was an exception, to tell a last-chance
// repeat from a first chance (the API does not say which one it is).
bool g_prev_exc = false;
uint32 g_prev_exc_code = 0;
ea_t g_prev_exc_ea = BADADDR;
thid_t g_prev_exc_tid = NO_THREAD;

const char *state_name(int st)
{
  return st == DSTATE_RUN ? "running" : st == DSTATE_SUSP ? "suspended" : "none";
}

void emit_json(Json &j)
{
  ev_emit(j.s);
}

Json begin_event(const char *kind)
{
  Json j;
  j.obj().kv("event", "debug").kv("kind", kind);
  return j;
}

bool ensure_loaded(std::string *why)
{
  if ( g_loaded )
    return true;
  if ( !load_debugger("win32", false) )
  {
    if ( why != nullptr )
      *why = "the win32 debugger module could not be loaded";
    return false;
  }
  g_loaded = true;
  return true;
}

std::string hex_bytes(const uchar *p, size_t n)
{
  static const char digits[] = "0123456789ABCDEF";
  std::string s;
  s.reserve(n * 2);
  for ( size_t i = 0; i < n; i++ )
  {
    s += digits[p[i] >> 4];
    s += digits[p[i] & 15];
  }
  return s;
}

bool parse_hex(const char *hex, std::vector<uchar> *out)
{
  auto nib = [](char c) -> int {
    if ( c >= '0' && c <= '9' ) return c - '0';
    if ( c >= 'a' && c <= 'f' ) return c - 'a' + 10;
    if ( c >= 'A' && c <= 'F' ) return c - 'A' + 10;
    return -1;
  };
  size_t n = strlen(hex);
  if ( n % 2 != 0 )
    return false;
  for ( size_t i = 0; i < n; i += 2 )
  {
    int a = nib(hex[i]), b = nib(hex[i + 1]);
    if ( a < 0 || b < 0 )
      return false;
    out->push_back((uchar)(a << 4 | b));
  }
  return true;
}

std::string module_base_name(const qstring &path)
{
  const char *s = path.c_str();
  const char *p = s;
  for ( const char *q = s; *q != '\0'; q++ )
    if ( *q == '\\' || *q == '/' )
      p = q + 1;
  return p;
}

// "name", "func+0xN" or "module+0xN" for an address; empty when unknown.
std::string symbolize(ea_t ea)
{
  qstring nm;
  if ( get_name(&nm, ea, GN_VISIBLE) > 0 && is_mapped(ea) )
    return qs(nm);
  func_t *f = get_func(ea);
  if ( f != nullptr )
  {
    char buf[32];
    qsnprintf(buf, sizeof(buf), "+0x%llX", (unsigned long long)(ea - f->start_ea));
    return short_name(f->start_ea) + buf;
  }
  if ( g_alive )
  {
    modinfo_t mi;
    if ( get_module_info(ea, &mi) )
    {
      char buf[32];
      qsnprintf(buf, sizeof(buf), "+0x%llX", (unsigned long long)(ea - mi.base));
      return module_base_name(mi.name) + buf;
    }
  }
  return "";
}

void write_state(Json &j)
{
  int st = get_process_state();
  j.obj().kv("state", state_name(st)).kv("pid", (uint64_t)(g_alive ? g_pid : 0));
  j.key("tid");
  if ( st == DSTATE_NOTASK )
    j.null();
  else
    j.u64((uint64_t)get_current_thread());
  ea_t ip = BADADDR, sp = BADADDR;
  if ( st == DSTATE_SUSP )
  {
    get_ip_val(&ip);
    get_sp_val(&sp);
  }
  j.key("ip").opt_u64(ip, BADADDR).key("sp").opt_u64(sp, BADADDR);
  j.key("orig_base").opt_u64(g_alive ? g_orig_base : BADADDR, BADADDR);
  j.kv("base", (uint64_t)get_imagebase());
  j.end_obj();
}

int reply_state()
{
  Json j;
  write_state(j);
  return ok(j);
}

// Compares the database base with the one last reported and, when the kernel
// moved it, tells the UI (every address it holds shifts by `delta`).
void check_rebase(bool restored)
{
  ea_t cur = get_imagebase();
  if ( g_last_base == BADADDR || cur == g_last_base )
    return;
  Json j = begin_event("rebased");
  j.kv("old_base", (uint64_t)g_last_base).kv("base", (uint64_t)cur);
  j.kvi("delta", (int64_t)((adiff_t)cur - (adiff_t)g_last_base));
  j.kvb("restored", restored).end_obj();
  emit_json(j);
  g_last_base = cur;
}

void restore_base()
{
  if ( g_orig_base == BADADDR )
    return;
  ea_t cur = get_imagebase();
  if ( cur != g_orig_base )
  {
    adiff_t delta = (adiff_t)g_orig_base - (adiff_t)cur;
    int rc = rebase_program(delta, MSF_FIXONCE | MSF_SILENT);
    if ( rc != MOVE_SEGM_OK )
    {
      Json j = begin_event("error");
      j.kv("text", "could not rebase the database back to its original base; do not save it").end_obj();
      emit_json(j);
    }
  }
  check_rebase(true);
}

void on_process_gone()
{
  if ( !g_alive )
    return;
  g_alive = false;
  g_pausing = false;
  restore_base();
  g_orig_base = BADADDR;
  g_last_base = BADADDR;
  g_reported = DSTATE_NOTASK;
  Json j = begin_event("state");
  j.kv("state", "none").key("ip").null().key("tid").null().kv("reason", "exited").end_obj();
  emit_json(j);
}

// Reports a change of the process state once.
void sync_state(const char *reason)
{
  int st = get_process_state();
  if ( st == DSTATE_NOTASK )
  {
    if ( g_alive )
      on_process_gone();
    return;
  }
  if ( st == g_reported )
    return;
  g_reported = st;
  if ( st == DSTATE_SUSP )
    g_pausing = false;
  Json j = begin_event("state");
  j.kv("state", state_name(st));
  ea_t ip = BADADDR;
  if ( st == DSTATE_SUSP )
    get_ip_val(&ip);
  j.key("ip").opt_u64(ip, BADADDR);
  j.key("tid").u64((uint64_t)get_current_thread());
  j.kv("reason", reason).end_obj();
  emit_json(j);
}

std::string exc_name(uint32 code, std::string *desc)
{
  excvec_t *v = retrieve_exceptions();
  if ( v != nullptr )
    for ( const exception_info_t &e : *v )
      if ( e.code == code )
      {
        if ( desc != nullptr )
          *desc = qs(e.desc);
        return qs(e.name);
      }
  return "";
}

void report_event(const debug_event_t &ev)
{
  const bool stopped = get_process_state() == DSTATE_SUSP;
  if ( ev.eid() != EXCEPTION && ev.eid() != STEP && ev.eid() != INFORMATION )
    g_prev_exc = false;
  const char *reason = "event";
  switch ( ev.eid() )
  {
    case PROCESS_STARTED:
    case PROCESS_ATTACHED:
    {
      g_pid = (int)ev.pid;
      const modinfo_t &m = ev.modinfo();
      Json j = begin_event(ev.eid() == PROCESS_STARTED ? "process_started" : "process_attached");
      j.kv("pid", (uint64_t)ev.pid).kv("name", qs(m.name));
      j.kv("base", (uint64_t)m.base).kv("size", (uint64_t)m.size).end_obj();
      emit_json(j);
      reason = "start";
      break;
    }
    case PROCESS_EXITED:
    {
      Json j = begin_event("process_exited");
      j.kv("pid", (uint64_t)ev.pid).kvi("exit_code", ev.exit_code()).end_obj();
      emit_json(j);
      break;
    }
    case PROCESS_DETACHED:
    {
      Json j = begin_event("process_detached");
      j.kv("pid", (uint64_t)ev.pid).end_obj();
      emit_json(j);
      break;
    }
    case THREAD_STARTED:
    {
      Json j = begin_event("thread_started");
      j.kv("tid", (uint64_t)ev.tid).kv("name", qs(ev.info())).end_obj();
      emit_json(j);
      reason = "thread";
      break;
    }
    case THREAD_EXITED:
    {
      Json j = begin_event("thread_exited");
      j.kv("tid", (uint64_t)ev.tid).kvi("exit_code", ev.exit_code()).end_obj();
      emit_json(j);
      reason = "thread";
      break;
    }
    case LIB_LOADED:
    {
      const modinfo_t &m = ev.modinfo();
      Json j = begin_event("lib_loaded");
      j.kv("name", qs(m.name)).kv("base", (uint64_t)m.base).kv("size", (uint64_t)m.size).end_obj();
      emit_json(j);
      reason = "library";
      break;
    }
    case LIB_UNLOADED:
    {
      Json j = begin_event("lib_unloaded");
      j.kv("name", qs(ev.info())).end_obj();
      emit_json(j);
      reason = "library";
      break;
    }
    case INFORMATION:
    {
      if ( ev.info().empty() )
        break;
      Json j = begin_event("output");
      j.kv("tid", (uint64_t)ev.tid).kv("text", qs(ev.info())).end_obj();
      emit_json(j);
      break;
    }
    case BREAKPOINT:
    {
      reason = "breakpoint";
      if ( !stopped )
        break;
      ea_t at = ev.bpt_ea();
      int hits = 0;
      if ( get_bpt(at, nullptr) )
        hits = ++g_hits[at];
      Json j = begin_event("breakpoint");
      j.kv("tid", (uint64_t)ev.tid).kv("ea", (uint64_t)at).kvi("hits", hits).end_obj();
      emit_json(j);
      break;
    }
    case STEP:
    {
      reason = "step";
      if ( !stopped )
        break;
      Json j = begin_event("step");
      j.kv("tid", (uint64_t)ev.tid).kv("ea", (uint64_t)ev.ea).end_obj();
      emit_json(j);
      break;
    }
    case EXCEPTION:
    {
      reason = "exception";
      const excinfo_t &x = ev.exc();
      std::string desc;
      std::string name = exc_name(x.code, &desc);
      Json j = begin_event("exception");
      j.kv("tid", (uint64_t)ev.tid).kv("ea", (uint64_t)ev.ea).kv("code", (uint64_t)x.code);
      j.kv("name", name).kv("desc", desc).kv("info", qs(x.info));
      j.kvb("can_continue", x.can_cont).kvb("handled", ev.handled).kvb("stopped", stopped);
      const bool again = g_prev_exc && g_prev_exc_code == x.code && g_prev_exc_ea == ev.ea
                      && g_prev_exc_tid == ev.tid;
      j.kvb("first_chance", !again).end_obj();
      emit_json(j);
      g_prev_exc = true;
      g_prev_exc_code = x.code;
      g_prev_exc_ea = ev.ea;
      g_prev_exc_tid = ev.tid;
      break;
    }
    default:
      break;
  }
  check_rebase(false);
  sync_state(reason);
}

// Waits one slice for a debugger event and reports it. Returns false when
// there was nothing to report (timeout or no process).
bool poll_once()
{
  dbg_event_code_t code = wait_for_next_event(WFNE_ANY | WFNE_USEC | WFNE_SILENT, 40000);
  if ( code == DEC_NOTASK )
  {
    on_process_gone();
    return false;
  }
  if ( code <= 0 )
    return false;
  const debug_event_t *ev = get_debug_event();
  if ( ev != nullptr )
    report_event(*ev);
  if ( ev != nullptr && ev->eid() == PROCESS_EXITED )
  {
    // The kernel updates its state after the event is handled.
    if ( get_process_state() == DSTATE_NOTASK )
      on_process_gone();
  }
  return true;
}

// Polls until the process state is `want`, nothing happens for a while or
// `ms` pass. Used by requests that must not return mid-transition.
void pump(int ms, int (*stop)(void))
{
  auto end = Clock::now() + std::chrono::milliseconds(ms);
  while ( Clock::now() < end )
  {
    if ( g_alive && get_process_state() == DSTATE_NOTASK )
    {
      on_process_gone();
      return;
    }
    if ( stop != nullptr && stop() )
      return;
    poll_once();
  }
}

int stop_when_gone(void)
{
  return !g_alive;
}

const char *bpt_kind(const bpt_t &b)
{
  switch ( b.type & 0xF )
  {
    case BPT_WRITE: return "write";
    case BPT_READ: return "read";
    case BPT_RDWR: return "rw";
    default: break;
  }
  return b.type & BPT_SOFT ? "soft" : "exec";
}

void write_bpt(Json &j, const bpt_t &b)
{
  j.obj().kv("ea", (uint64_t)b.ea).kvb("enabled", b.enabled()).kv("kind", bpt_kind(b));
  j.kv("size", (uint64_t)b.get_size()).kv("cond", qs(b.cndbody));
  const char *lang = b.get_cnd_elang();
  j.kv("lang", lang != nullptr ? lang : "IDC");
  auto it = g_hits.find(b.ea);
  j.kvi("hits", it == g_hits.end() ? 0 : it->second);
  j.kvb("active", b.is_active()).kvb("bad", b.badbpt());
  std::string sym = symbolize(b.ea);
  j.kv("symbol", sym);
  j.end_obj();
}

int reply_bpts()
{
  Json j;
  j.arr();
  int n = get_bpt_qty();
  for ( int i = 0; i < n; i++ )
  {
    bpt_t b;
    if ( getn_bpt(i, &b) && b.listbpt() )
      write_bpt(j, b);
  }
  j.end_arr();
  return ok(j);
}

#define REQUIRE_SUSPENDED() do { \
    if ( !g_alive || get_process_state() != DSTATE_SUSP ) \
      return fail("the process is not suspended"); \
  } while ( 0 )

#define REQUIRE_LOADED() do { \
    std::string why_; \
    if ( !ensure_loaded(&why_) ) \
      return fail("%s", why_.c_str()); \
  } while ( 0 )
} // namespace

//--------------------------------------------------------------------------
bool dbg_process_alive()
{
  return g_alive && get_process_state() != DSTATE_NOTASK;
}

bool dbg_db_rebased()
{
  return g_orig_base != BADADDR && get_imagebase() != g_orig_base;
}

bool dbg_shutdown()
{
  if ( g_alive && get_process_state() != DSTATE_NOTASK )
  {
    exit_process();
    pump(4000, stop_when_gone);
  }
  if ( g_alive )
    on_process_gone();
  return !dbg_db_rebased();
}

extern "C" {

// 1 when the process runs (the host loop must keep polling), 0 otherwise.
int izs_dbg_running(void)
{
  if ( !g_alive )
    return 0;
  int st = get_process_state();
  if ( st == DSTATE_NOTASK )
  {
    on_process_gone();
    return 0;
  }
  return st == DSTATE_RUN ? 1 : 0;
}

void izs_dbg_poll(void)
{
  if ( !g_alive )
    return;
  try
  {
    for ( int i = 0; i < 64 && g_alive; i++ )
    {
      if ( get_process_state() != DSTATE_RUN )
        break;
      if ( !poll_once() )
        break;
    }
    if ( g_alive && g_pausing && get_process_state() == DSTATE_RUN
      && Clock::now() > g_pause_deadline )
    {
      g_pausing = false;
      Json j = begin_event("pause_timeout");
      j.end_obj();
      emit_json(j);
    }
  }
  catch ( ... )
  {
  }
}

// flags: 1 stop at entry, 2 stop on library events, 4 stop on thread events,
// 8 disable ASLR.
int izs_dbg_start(const char *path, const char *args, const char *cwd, uint32_t flags)
{
  REQUIRE_DB();
  GUARD_BEGIN
  REQUIRE_LOADED();
  if ( g_alive )
    return fail("a process is already being debugged");
  uint opts = 0;
  if ( flags & 1 ) opts |= DOPT_ENTRY_BPT;
  if ( flags & 2 ) opts |= DOPT_LIB_BPT;
  if ( flags & 4 ) opts |= DOPT_THREAD_BPT;
  if ( flags & 8 ) opts |= DOPT_DISABLE_ASLR;
  // Never fetch symbols (DOPT_LOAD_DINFO stays off): no network access.
  set_debugger_options(opts);
  qstring exe = path;
  if ( exe.empty() )
  {
    char buf[QMAXPATH];
    buf[0] = '\0';
    get_input_file_path(buf, sizeof(buf));
    exe = buf;
  }
  g_orig_base = get_imagebase();
  g_last_base = g_orig_base;
  g_hits.clear();
  g_prev_exc = false;
  g_reported = DSTATE_NOTASK;
  int rc = start_process(exe.c_str(), args, cwd[0] != '\0' ? cwd : nullptr);
  if ( rc != 1 )
  {
    g_orig_base = BADADDR;
    g_last_base = BADADDR;
    return fail(rc == 0 ? "start was cancelled" : "the process could not be started");
  }
  g_alive = true;
  g_reported = get_process_state();
  Json j = begin_event("state");
  j.kv("state", state_name(g_reported)).key("ip").null().key("tid").null().kv("reason", "start").end_obj();
  emit_json(j);
  return reply_state();
  GUARD_END
}

int izs_dbg_processes(void)
{
  REQUIRE_DB();
  GUARD_BEGIN
  REQUIRE_LOADED();
  procinfo_vec_t procs;
  get_processes(&procs);
  Json j;
  j.arr();
  for ( const process_info_t &p : procs )
    j.obj().kv("pid", (uint64_t)p.pid).kv("name", qs(p.name)).end_obj();
  j.end_arr();
  return ok(j);
  GUARD_END
}

int izs_dbg_attach(uint32_t pid, uint32_t flags)
{
  REQUIRE_DB();
  GUARD_BEGIN
  REQUIRE_LOADED();
  if ( g_alive )
    return fail("a process is already being debugged");
  uint opts = 0;
  if ( flags & 2 ) opts |= DOPT_LIB_BPT;
  if ( flags & 4 ) opts |= DOPT_THREAD_BPT;
  set_debugger_options(opts);
  g_orig_base = get_imagebase();
  g_last_base = g_orig_base;
  g_hits.clear();
  g_reported = DSTATE_NOTASK;
  int rc = attach_process((pid_t)pid, -1);
  if ( rc != 1 )
  {
    g_orig_base = BADADDR;
    g_last_base = BADADDR;
    return fail("could not attach to process %u", pid);
  }
  g_alive = true;
  g_pid = (int)pid;
  g_reported = get_process_state();
  return reply_state();
  GUARD_END
}

int izs_dbg_state(void)
{
  REQUIRE_DB();
  GUARD_BEGIN
  return reply_state();
  GUARD_END
}

int izs_dbg_detach(void)
{
  REQUIRE_DB();
  GUARD_BEGIN
  if ( !g_alive )
    return fail("no process");
  detach_process();
  pump(4000, stop_when_gone);
  return reply_state();
  GUARD_END
}

int izs_dbg_terminate(void)
{
  REQUIRE_DB();
  GUARD_BEGIN
  if ( !g_alive )
    return fail("no process");
  exit_process();
  pump(4000, stop_when_gone);
  return reply_state();
  GUARD_END
}

// Resume requests return at once; the next stop arrives as events.
int izs_dbg_resume(int how, uint64_t ea)
{
  REQUIRE_DB();
  GUARD_BEGIN
  REQUIRE_SUSPENDED();
  bool done = false;
  switch ( how )
  {
    case 0: done = continue_process(); break;
    case 1: done = step_into(); break;
    case 2: done = step_over(); break;
    case 3: done = step_until_ret(); break;
    case 4: done = run_to((ea_t)ea); break;
    default: return fail("unknown resume mode");
  }
  if ( !done )
    return fail("the debugger refused to resume");
  sync_state(how == 0 ? "continue" : "step");
  return reply_state();
  GUARD_END
}

int izs_dbg_pause(uint32_t timeout_ms)
{
  REQUIRE_DB();
  GUARD_BEGIN
  if ( !g_alive )
    return fail("no process");
  if ( get_process_state() != DSTATE_RUN )
    return reply_state();
  // The request form queues the suspension; the blocking one can wait for
  // ever on a target that never reaches a safe point.
  request_suspend_process();
  run_requests();
  g_pausing = true;
  g_pause_deadline = Clock::now() + std::chrono::milliseconds(timeout_ms);
  return reply_state();
  GUARD_END
}

//--------------------------------------------------------------------------
int izs_dbg_bpts(void)
{
  REQUIRE_DB();
  GUARD_BEGIN
  REQUIRE_LOADED();
  return reply_bpts();
  GUARD_END
}

// kind: soft, exec, write, read, rw.
int izs_dbg_bpt_add(uint64_t ea, const char *kind, uint32_t size, const char *cond, const char *lang)
{
  REQUIRE_DB();
  GUARD_BEGIN
  REQUIRE_LOADED();
  bpttype_t type = BPT_DEFAULT;
  if ( strcmp(kind, "exec") == 0 ) type = BPT_EXEC;
  else if ( strcmp(kind, "write") == 0 ) type = BPT_WRITE;
  else if ( strcmp(kind, "read") == 0 ) type = BPT_READ;
  else if ( strcmp(kind, "rw") == 0 ) type = BPT_RDWR;
  if ( get_bpt((ea_t)ea, nullptr) )
    return fail("there is a breakpoint at this address already");
  if ( !add_bpt((ea_t)ea, size, type) )
    return fail("the breakpoint could not be set");
  if ( cond[0] != '\0' )
  {
    bpt_t b;
    if ( get_bpt((ea_t)ea, &b) )
    {
      b.cndbody = cond;
      if ( lang[0] != '\0' )
        b.set_cnd_elang(lang);
      update_bpt(&b);
    }
  }
  return reply_bpts();
  GUARD_END
}

int izs_dbg_bpt_del(uint64_t ea)
{
  REQUIRE_DB();
  GUARD_BEGIN
  REQUIRE_LOADED();
  if ( !del_bpt((ea_t)ea) )
    return fail("there is no breakpoint at this address");
  g_hits.erase((ea_t)ea);
  return reply_bpts();
  GUARD_END
}

int izs_dbg_bpt_enable(uint64_t ea, bool on)
{
  REQUIRE_DB();
  GUARD_BEGIN
  REQUIRE_LOADED();
  if ( !enable_bpt((ea_t)ea, on) )
    return fail("there is no breakpoint at this address");
  return reply_bpts();
  GUARD_END
}

int izs_dbg_bpt_cond(uint64_t ea, const char *cond, const char *lang)
{
  REQUIRE_DB();
  GUARD_BEGIN
  REQUIRE_LOADED();
  bpt_t b;
  if ( !get_bpt((ea_t)ea, &b) )
    return fail("there is no breakpoint at this address");
  b.cndbody = cond;
  if ( lang[0] != '\0' )
    b.set_cnd_elang(lang);
  if ( !update_bpt(&b) )
    return fail("the condition was rejected");
  return reply_bpts();
  GUARD_END
}

//--------------------------------------------------------------------------
int izs_dbg_regs(bool all)
{
  REQUIRE_DB();
  GUARD_BEGIN
  REQUIRE_SUSPENDED();
  Json j;
  j.arr();
  if ( dbg == nullptr )
    return fail("no debugger module");
  for ( int i = 0; i < dbg->nregisters; i++ )
  {
    register_info_t &ri = dbg->regs(i);
    int cls = 0;
    while ( cls < 31 && !(ri.register_class & (1 << cls)) )
      cls++;
    if ( !all && !((1 << cls) & dbg->default_regclasses) )
      continue;
    regval_t rv;
    if ( !get_reg_val(ri.name, &rv) || rv.rvtype == RVT_UNAVAILABLE )
      continue;
    size_t bytes = get_dtype_size(ri.dtype);
    std::string hex;
    if ( rv.rvtype == RVT_INT )
    {
      uchar buf[8];
      uint64 v = rv.ival;
      for ( int k = 0; k < 8; k++ )
        buf[7 - k] = (uchar)(v >> (8 * k));
      size_t n = bytes < 8 && bytes > 0 ? bytes : 8;
      hex = hex_bytes(buf + 8 - n, n);
    }
    else
    {
      const bytevec_t &bv = rv.bytes();
      std::vector<uchar> be(bv.size());
      for ( size_t k = 0; k < bv.size(); k++ )
        be[bv.size() - 1 - k] = bv[k];
      hex = hex_bytes(be.data(), be.size());
    }
    j.obj().kv("name", ri.name).kv("bits", (uint64_t)bytes * 8);
    j.kv("class", dbg->regclasses != nullptr ? dbg->regclasses[cls] : "");
    j.kv("hex", hex).kvb("float", rv.rvtype == RVT_FLOAT);
    j.kvb("ip", (ri.flags & REGISTER_IP) != 0).kvb("sp", (ri.flags & REGISTER_SP) != 0);
    j.kvb("fp", (ri.flags & REGISTER_FP) != 0).kvb("address", (ri.flags & REGISTER_ADDRESS) != 0);
    j.kvb("readonly", (ri.flags & REGISTER_READONLY) != 0);
    j.key("bit_names").arr();
    if ( ri.bit_strings != nullptr && rv.rvtype == RVT_INT )
      for ( size_t b = 0; b < bytes * 8; b++ )
        if ( ri.bit_strings[b] != nullptr && ri.bit_strings[b][0] != '\0' )
          j.arr().u64(b).str(ri.bit_strings[b]).end_arr();
    j.end_arr();
    j.end_obj();
  }
  j.end_arr();
  return ok(j);
  GUARD_END
}

int izs_dbg_set_reg(const char *name, uint64_t value)
{
  REQUIRE_DB();
  GUARD_BEGIN
  REQUIRE_SUSPENDED();
  if ( !is_reg_integer(name) )
    return fail("only integer registers can be written");
  if ( !set_reg_val(name, (uint64)value) )
    return fail("could not write register %s", name);
  return ok_null();
  GUARD_END
}

int izs_dbg_mem_read(uint64_t ea, uint32_t len)
{
  REQUIRE_DB();
  GUARD_BEGIN
  REQUIRE_SUSPENDED();
  if ( len > (1u << 16) )
    len = 1u << 16;
  std::vector<uchar> buf(len);
  ssize_t got = read_dbg_memory((ea_t)ea, buf.data(), len);
  static const char digits[] = "0123456789ABCDEF";
  std::string hex;
  hex.reserve(len * 2);
  std::vector<bool> valid(len, got == (ssize_t)len);
  if ( got != (ssize_t)len )
  {
    // Partial or failed read: probe in small pieces so a readable prefix or
    // a readable tail still shows, and unreadable bytes show as "??".
    for ( uint32_t off = 0; off < len; off += 16 )
    {
      uint32_t n = qmin(16u, len - off);
      ssize_t g = read_dbg_memory((ea_t)ea + off, buf.data() + off, n);
      for ( uint32_t k = 0; k < n; k++ )
        valid[off + k] = g == (ssize_t)n;
    }
  }
  // Show original bytes under our own software breakpoints.
  int nb = get_bpt_qty();
  for ( int i = 0; i < nb; i++ )
  {
    bpt_t b;
    if ( getn_bpt(i, &b) && b.enabled() && !b.is_hwbpt() && b.ea >= (ea_t)ea && b.ea < (ea_t)ea + len
      && valid[b.ea - (ea_t)ea] && buf[b.ea - (ea_t)ea] == 0xCC && is_loaded(b.ea) )
      buf[b.ea - (ea_t)ea] = get_byte(b.ea);
  }
  for ( uint32_t i = 0; i < len; i++ )
  {
    if ( valid[i] )
    {
      hex += digits[buf[i] >> 4];
      hex += digits[buf[i] & 15];
    }
    else
      hex += "??";
  }
  Json j;
  j.obj().kv("ea", ea).kv("hex", hex).end_obj();
  return ok(j);
  GUARD_END
}

int izs_dbg_mem_write(uint64_t ea, const char *hex)
{
  REQUIRE_DB();
  GUARD_BEGIN
  REQUIRE_SUSPENDED();
  std::vector<uchar> bytes;
  if ( !parse_hex(hex, &bytes) || bytes.empty() )
    return fail("expected an even number of hex digits");
  ssize_t put = write_dbg_memory((ea_t)ea, bytes.data(), bytes.size());
  if ( put != (ssize_t)bytes.size() )
    return fail("could only write %d of %d bytes", (int)put, (int)bytes.size());
  Json j;
  j.u64(bytes.size());
  return ok(j);
  GUARD_END
}

int izs_dbg_threads(void)
{
  REQUIRE_DB();
  GUARD_BEGIN
  if ( !g_alive )
    return fail("no process");
  Json j;
  j.arr();
  thid_t cur = get_current_thread();
  int n = get_thread_qty();
  bool suspended = get_process_state() == DSTATE_SUSP;
  for ( int i = 0; i < n; i++ )
  {
    thid_t tid = getn_thread(i);
    const char *nm = getn_thread_name(i);
    // Each thread's instruction pointer: select it, read it, and put the
    // selection back at the end.
    uint64_t ip = 0;
    if ( suspended && tid != cur && select_thread(tid) )
    {
      regval_t rv;
      if ( get_ip_val(&rv.ival) )
        ip = rv.ival;
    }
    else if ( suspended && tid == cur )
    {
      uval_t v = 0;
      if ( get_ip_val(&v) )
        ip = v;
    }
    j.obj().kv("tid", (uint64_t)tid).kv("name", nm != nullptr ? nm : "").kvb("current", tid == cur);
    j.kv("ip", ip).end_obj();
  }
  if ( suspended && n > 1 && get_current_thread() != cur )
    select_thread(cur);
  j.end_arr();
  return ok(j);
  GUARD_END
}

int izs_dbg_select_thread(uint32_t tid)
{
  REQUIRE_DB();
  GUARD_BEGIN
  REQUIRE_SUSPENDED();
  if ( !select_thread((thid_t)tid) )
    return fail("could not select thread %u", tid);
  return reply_state();
  GUARD_END
}

int izs_dbg_modules(void)
{
  REQUIRE_DB();
  GUARD_BEGIN
  if ( !g_alive )
    return fail("no process");
  Json j;
  j.arr();
  modinfo_t mi;
  for ( bool more = get_first_module(&mi); more; more = get_next_module(&mi) )
    j.obj().kv("name", qs(mi.name)).kv("base", (uint64_t)mi.base).kv("size", (uint64_t)mi.size).end_obj();
  j.end_arr();
  return ok(j);
  GUARD_END
}

int izs_dbg_stack(void)
{
  REQUIRE_DB();
  GUARD_BEGIN
  REQUIRE_SUSPENDED();
  call_stack_t trace;
  if ( !collect_stack_trace(get_current_thread(), &trace) )
    return fail("the call stack could not be collected");
  Json j;
  j.arr();
  for ( const call_stack_info_t &fr : trace )
  {
    j.obj().kv("ea", (uint64_t)fr.callea).kv("func", (uint64_t)fr.funcea).kv("fp", (uint64_t)fr.fp);
    j.kv("symbol", symbolize(fr.callea)).end_obj();
  }
  j.end_arr();
  return ok(j);
  GUARD_END
}

// Comma-separated hex addresses in, an array of names (or null) out.
int izs_dbg_symbolize(const char *eas)
{
  REQUIRE_DB();
  GUARD_BEGIN
  Json j;
  j.arr();
  const char *p = eas;
  while ( *p != '\0' )
  {
    char *end = nullptr;
    unsigned long long v = strtoull(p, &end, 16);
    std::string s = symbolize((ea_t)v);
    if ( s.empty() )
      j.null();
    else
      j.str(s);
    p = end;
    while ( *p == ',' || *p == ' ' )
      p++;
    if ( end == p && *p != '\0' )
      break;
  }
  j.end_arr();
  return ok(j);
  GUARD_END
}

// IDC expression with registers, names and numbers.
int izs_dbg_eval(const char *expr)
{
  REQUIRE_DB();
  GUARD_BEGIN
  idc_value_t rv;
  qstring err;
  ea_t here = BADADDR;
  if ( g_alive && get_process_state() == DSTATE_SUSP )
    get_ip_val(&here);
  if ( !eval_expr(&rv, here, expr, &err) )
    return fail("%s", err.c_str());
  Json j;
  if ( rv.vtype == VT_LONG )
    j.u64((uint64_t)rv.num);
  else if ( rv.vtype == VT_INT64 )
    j.u64((uint64_t)rv.i64);
  else
    return fail("the expression is not a number");
  return ok(j);
  GUARD_END
}

//--------------------------------------------------------------------------
int izs_dbg_exceptions(void)
{
  REQUIRE_DB();
  GUARD_BEGIN
  REQUIRE_LOADED();
  excvec_t *v = retrieve_exceptions();
  Json j;
  j.arr();
  if ( v != nullptr )
    for ( const exception_info_t &e : *v )
    {
      j.obj().kv("code", (uint64_t)e.code).kv("name", qs(e.name)).kv("desc", qs(e.desc));
      j.kvb("stop", e.break_on()).kvb("pass", !e.handle()).end_obj();
    }
  j.end_arr();
  return ok(j);
  GUARD_END
}

int izs_dbg_set_exception(uint32_t code, bool stop, bool pass)
{
  REQUIRE_DB();
  GUARD_BEGIN
  REQUIRE_LOADED();
  excvec_t *v = retrieve_exceptions();
  if ( v == nullptr )
    return fail("no exception table");
  bool found = false;
  for ( exception_info_t &e : *v )
    if ( e.code == code )
    {
      setflag(e.flags, EXC_BREAK, stop);
      setflag(e.flags, EXC_HANDLE, !pass);
      found = true;
    }
  if ( !found )
  {
    // A code IDA has no entry for yet (a program's own, e.g. 0xE0...).
    char name[32];
    qsnprintf(name, sizeof(name), "EXC_%08X", code);
    uint32 flags = (stop ? EXC_BREAK : 0) | (pass ? 0 : EXC_HANDLE);
    const char *why = define_exception(code, name, "", flags);
    if ( why != nullptr )
      return fail("%s", why);
  }
  store_exceptions();
  return izs_dbg_exceptions();
  GUARD_END
}

} // extern "C"
