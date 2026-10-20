// extern "C" surface over the IDA SDK for ida-host. Every function must be
// called from the thread that called izs_init (idalib is single-threaded).
//
// Convention: each izs_* command returns 0 on success and leaves a JSON value
// in the result buffer, or returns non-zero and leaves an error message there.
// Rust reads the buffer with izs_result() before the next call.

#include <pro.h>
#include <ida.hpp>
#include <idp.hpp>
#include <auto.hpp>
#include <bytes.hpp>
#include <diskio.hpp>
#include <entry.hpp>
#include <funcs.hpp>
#include <gdl.hpp>
#include <kernwin.hpp>
#include <lines.hpp>
#include <loader.hpp>
#include <nalt.hpp>
#include <name.hpp>
#include <demangle.hpp>
#include <search.hpp>
#include <segment.hpp>
#include <strlist.hpp>
#include <typeinf.hpp>
#include <ua.hpp>
#include <undo.hpp>
#include <xref.hpp>
#include <hexrays.hpp>
#include <idalib.hpp>

#include <string>

#include "common.hpp"
#include "debug.hpp"
#include "events.hpp"

std::string short_name(ea_t ea)
{
  qstring q = get_short_name(ea);
  if ( q.empty() )
    q = get_name(ea);
  return qs(q);
}

// The short demangled form of `name`, or empty when it is not a mangled name.
static std::string demangled_short(const char *name)
{
  qstring out;
  if ( name == nullptr || demangle_name(&out, name, MNG_SHORT_FORM) < 0 )
    return std::string();
  return qs(out);
}

static std::string seg_prefix(ea_t ea)
{
  qstring segname;
  segment_t *s = getseg(ea);
  if ( s != nullptr )
    get_segm_name(&segname, s);
  else
    segname = "?";
  char buf[64];
  qsnprintf(buf, sizeof(buf), ":%016llX", (unsigned long long)ea);
  return qs(segname) + buf;
}

//--------------------------------------------------------------------------
// Listing lines via linearray_t, the kernel's own line generator.

static void emit_line(Json &j, ea_t ea, const qstring &line, color_t pfx)
{
  j.obj()
    .kv("ea", (uint64_t)ea)
    .kv("tagged", qs(line))
    .kv("prefix", seg_prefix(ea))
    .kv("prefix_color", (uint64_t)pfx)
    .end_obj();
}

// Emits lines from `start` until at least `count` lines were produced and an
// item boundary is reached, or until `stop` (exclusive) if not BADADDR.
// Returns the first item not emitted, or BADADDR at the end of the database.
static ea_t listing_down(Json &j, ea_t start, uint32 count, ea_t stop, bool *at_end)
{
  idaplace_t pl(start, 0);
  linearray_t ln(nullptr);
  ln.set_place(&pl);
  uint32 produced = 0;
  ea_t last_item = BADADDR;
  *at_end = false;
  for ( ;; )
  {
    place_t *p = ln.get_place();
    ea_t ea = p != nullptr ? p->toea() : BADADDR;
    if ( stop != BADADDR && ea >= stop )
      return ea;
    if ( produced >= count && ea != last_item )
      return ea;
    color_t pfx = (color_t)ln.get_pfx_color();
    const qstring *s = ln.down();
    if ( s == nullptr )
    {
      *at_end = true;
      return BADADDR;
    }
    emit_line(j, ea, *s, pfx);
    produced++;
    last_item = ea;
  }
}

//--------------------------------------------------------------------------
extern "C" {

const char *izs_result(size_t *len)
{
  *len = g_result.size();
  return g_result.data();
}

int izs_init(bool binary_loader)
{
  // idalib accepts exactly one switch here, as a single token: -T forces
  // the loader ("Binary file" loads the input as a raw blob).
  char arg0[] = "ida";
  char arg1[] = "-TBinary file";
  char *argv[] = { arg0, arg1, nullptr };
  int rc = binary_loader ? init_library(2, argv) : init_library();
  if ( rc != 0 )
    return fail("init_library failed with code %d", rc);
  // IDA's messages reach the UI through the ui_msg hook as `log` events;
  // stderr stays for what happens outside it.
  enable_console_messages(false);
  ev_init();
  return ok_null();
}

void izs_set_emitter(void (*fn)(const char *, size_t))
{
  ev_set_emitter(fn);
}

// Brackets one request: the origin tags the database changes it causes and
// the end flushes the events that are still pending.
void izs_begin_request(int origin)
{
  ev_begin_request(origin);
}

void izs_end_request(void)
{
  ev_end_request();
}

static int db_info()
{
  Json j;
  char buf[QMAXPATH];
  j.obj();
  buf[0] = '\0';
  get_input_file_path(buf, sizeof(buf));
  j.kv("input_path", buf);
  const char *idb = get_path(PATH_TYPE_IDB);
  j.kv("idb_path", idb != nullptr ? idb : "");
  qstring proc = inf_get_procname();
  j.kv("processor", qs(proc));
  j.kv("bitness", (uint64_t)(inf_is_64bit() ? 64 : inf_is_32bit_exactly() ? 32 : 16));
  j.kv("min_ea", (uint64_t)inf_get_min_ea());
  j.kv("max_ea", (uint64_t)inf_get_max_ea());
  j.kv("image_base", (uint64_t)get_imagebase());
  ea_t start = inf_get_start_ea();
  j.key("entry").opt_u64(start, BADADDR);
  j.kvb("hexrays", g_hexrays);
  buf[0] = '\0';
  get_file_type_name(buf, sizeof(buf));
  j.kv("file_type", buf);
  j.end_obj();
  return ok(j);
}

// base == BADADDR keeps the loader's addresses. A rebase has to happen
// before analysis, so with one the database is opened without analysis and
// the queue is run afterwards.
// Display preferences ida-zui changes in the kernel's `inf` (listing columns,
// the address prefix). They are put back before the database is written, so
// they never leak into the user's .i64.
static struct
{
  bool have = false;
  uchar indent = 0;
  uchar cmt_indent = 0;
  bool show_pref = false;
} g_orig_prefs;

static void remember_original_prefs()
{
  g_orig_prefs.indent = inf_get_indent();
  g_orig_prefs.cmt_indent = inf_get_cmt_indent();
  g_orig_prefs.show_pref = inf_show_line_pref();
  g_orig_prefs.have = true;
}

static void restore_original_prefs()
{
  if ( !g_orig_prefs.have )
    return;
  inf_set_indent(g_orig_prefs.indent);
  inf_set_cmt_indent(g_orig_prefs.cmt_indent);
  inf_set_show_line_pref(g_orig_prefs.show_pref);
}

int izs_open(const char *utf8_path, bool run_auto, uint64_t base)
{
  GUARD_BEGIN
  if ( g_open )
    return fail("a database is already open");
  bool rebase = base != BADADDR;
  // Analysis runs below, in steps, so the UI can follow it. For timing
  // comparisons IZS_AUTO_MODE=native lets open_database run it (no progress
  // events) and IZS_AUTO_MODE=wait runs IDA's own auto_wait() after opening.
  qstring mode;
  qgetenv("IZS_AUTO_MODE", &mode);
  bool native = mode == "native";
  int rc = open_database(utf8_path, run_auto && native);
  if ( rc != 0 )
    return fail("open_database failed with code %d", rc);
  g_open = true;
  remember_original_prefs();
  if ( rebase )
  {
    adiff_t delta = (adiff_t)base - (adiff_t)inf_get_min_ea();
    if ( delta != 0 )
    {
      int mrc = rebase_program(delta, MSF_FIXONCE);
      if ( mrc != MOVE_SEGM_OK )
        return fail("rebase to %llX failed with code %d", (unsigned long long)base, mrc);
    }
  }
  if ( run_auto && !native )
  {
    if ( mode == "wait" )
    {
      enable_auto(true);
      auto_wait();
    }
    else
    {
      ev_run_auto(true);
    }
  }
  else if ( run_auto )
  {
    auto_wait();
  }
  // The UI draws its own address column.
  inf_set_show_line_pref(false);
  g_hexrays = init_hexrays_plugin();
  // Changes are reported from here on, not while the database is built.
  ev_hook_idb();
  return db_info();
  GUARD_END
}

// Works without a database.
int izs_version(void)
{
  GUARD_BEGIN
  int major = 0, minor = 0, build = 0;
  get_library_version(major, minor, build);
  Json j;
  j.obj();
  char ver[64];
  qsnprintf(ver, sizeof(ver), "%d.%d.%d", major, minor, build);
  j.kv("ida_version", ver);
  const char *dir = idadir(nullptr);
  j.kv("ida_dir", dir != nullptr ? dir : "");
  const char *user = get_user_idadir();
  j.kv("user_dir", user != nullptr ? user : "");
  j.end_obj();
  return ok(j);
  GUARD_END
}

int izs_info(void)
{
  REQUIRE_DB();
  return db_info();
}

int izs_save(void)
{
  REQUIRE_DB();
  GUARD_BEGIN
  // A database rebased to the debuggee must never reach the disk.
  if ( dbg_process_alive() )
    return fail("a debugging session is active; terminate it before saving");
  if ( dbg_db_rebased() )
    return fail("the database is rebased to a debuggee; not saving");
  // Our display columns are applied again once the file is written.
  const uchar indent = inf_get_indent();
  const uchar cmt_indent = inf_get_cmt_indent();
  const bool show_pref = inf_show_line_pref();
  restore_original_prefs();
  const bool saved = save_database(nullptr, 0);
  inf_set_indent(indent);
  inf_set_cmt_indent(cmt_indent);
  inf_set_show_line_pref(show_pref);
  if ( !saved )
    return fail("save_database failed");
  return ok_null();
  GUARD_END
}

// Instruction and comment columns of the generated listing lines. IDA's
// defaults (16 and 40) waste width in narrow panes and graph blocks.
int izs_set_listing_options(uint32_t indent, uint32_t cmt_indent)
{
  REQUIRE_DB();
  GUARD_BEGIN
  if ( indent > 64 || cmt_indent > 128 )
    return fail("indent out of range");
  inf_set_indent((uchar)indent);
  inf_set_cmt_indent((uchar)cmt_indent);
  return ok_null();
  GUARD_END
}

void izs_close(bool save)
{
  if ( g_open )
  {
    // End a debugging session first: it leaves the database rebased.
    if ( !dbg_shutdown() )
      save = false;
    if ( g_hexrays )
      term_hexrays_plugin();
    ev_unhook_idb();
    if ( save )
      restore_original_prefs();
    close_database(save);
    g_open = false;
  }
  ok_null();
}

//--------------------------------------------------------------------------
int izs_functions(void)
{
  REQUIRE_DB();
  GUARD_BEGIN
  Json j;
  j.arr();
  size_t n = get_func_qty();
  for ( size_t i = 0; i < n; i++ )
  {
    func_t *f = getn_func(i);
    if ( f == nullptr )
      continue;
    j.obj()
      .kv("start", (uint64_t)f->start_ea)
      .kv("end", (uint64_t)f->end_ea)
      .kv("name", short_name(f->start_ea))
      .kvb("library", (f->flags & FUNC_LIB) != 0)
      .kvb("thunk", (f->flags & FUNC_THUNK) != 0)
      .end_obj();
  }
  j.end_arr();
  return ok(j);
  GUARD_END
}

int izs_names(void)
{
  REQUIRE_DB();
  GUARD_BEGIN
  Json j;
  j.arr();
  size_t n = get_nlist_size();
  for ( size_t i = 0; i < n; i++ )
  {
    ea_t ea = get_nlist_ea(i);
    j.obj().kv("ea", (uint64_t)ea).kv("name", short_name(ea)).end_obj();
  }
  j.end_arr();
  return ok(j);
  GUARD_END
}

int izs_strings(void)
{
  REQUIRE_DB();
  GUARD_BEGIN
  build_strlist();
  Json j;
  j.arr();
  size_t n = get_strlist_qty();
  for ( size_t i = 0; i < n; i++ )
  {
    string_info_t si;
    if ( !get_strlist_item(&si, i) )
      continue;
    qstring text;
    get_strlit_contents(&text, si.ea, si.length, si.type);
    const char *kind = "C";
    switch ( si.type & STRWIDTH_MASK )
    {
      case STRWIDTH_2B: kind = "UTF-16"; break;
      case STRWIDTH_4B: kind = "UTF-32"; break;
    }
    j.obj()
      .kv("ea", (uint64_t)si.ea)
      .kv("len", (uint64_t)si.length)
      .kv("kind", kind)
      .kv("text", qs(text))
      .end_obj();
  }
  j.end_arr();
  return ok(j);
  GUARD_END
}

struct import_ctx_t
{
  Json *j;
  const qstring *module;
};

static int idaapi import_cb(ea_t ea, const char *name, uval_t ord, void *param)
{
  import_ctx_t *ctx = (import_ctx_t *)param;
  qstring nm;
  if ( name != nullptr )
    nm = name;
  else
    nm.sprnt("#%llu", (unsigned long long)ord);
  ctx->j->obj()
    .kv("ea", (uint64_t)ea)
    .kv("module", qs(*ctx->module))
    .kv("name", qs(nm))
    .kv("short", demangled_short(nm.c_str()))
    .kv("ordinal", (uint64_t)ord)
    .end_obj();
  return 1;
}

int izs_imports(void)
{
  REQUIRE_DB();
  GUARD_BEGIN
  Json j;
  j.arr();
  uint n = get_import_module_qty();
  for ( uint i = 0; i < n; i++ )
  {
    qstring module;
    get_import_module_name(&module, i);
    import_ctx_t ctx { &j, &module };
    enum_import_names(i, import_cb, &ctx);
  }
  j.end_arr();
  return ok(j);
  GUARD_END
}

int izs_exports(void)
{
  REQUIRE_DB();
  GUARD_BEGIN
  Json j;
  j.arr();
  size_t n = get_entry_qty();
  for ( size_t i = 0; i < n; i++ )
  {
    uval_t ord = get_entry_ordinal(i);
    ea_t ea = get_entry(ord);
    qstring name;
    get_entry_name(&name, ord);
    j.obj().kv("ea", (uint64_t)ea).kv("ordinal", (uint64_t)ord).kv("name", qs(name)).kv("short", demangled_short(name.c_str())).end_obj();
  }
  j.end_arr();
  return ok(j);
  GUARD_END
}

int izs_segments(void)
{
  REQUIRE_DB();
  GUARD_BEGIN
  Json j;
  j.arr();
  int n = get_segm_qty();
  for ( int i = 0; i < n; i++ )
  {
    segment_t *s = getnseg(i);
    if ( s == nullptr )
      continue;
    qstring name, cls;
    get_visible_segm_name(&name, s);
    get_segm_class(&cls, s);
    char perm[4] = {
      (s->perm & SEGPERM_READ) ? 'R' : '-',
      (s->perm & SEGPERM_WRITE) ? 'W' : '-',
      (s->perm & SEGPERM_EXEC) ? 'X' : '-',
      '\0',
    };
    j.obj()
      .kv("start", (uint64_t)s->start_ea)
      .kv("end", (uint64_t)s->end_ea)
      .kv("name", qs(name))
      .kv("class", qs(cls))
      .kv("perm", perm)
      .kv("bitness", (uint64_t)(s->bitness == 2 ? 64 : s->bitness == 1 ? 32 : 16))
      .end_obj();
  }
  j.end_arr();
  return ok(j);
  GUARD_END
}

int izs_local_types(void)
{
  REQUIRE_DB();
  GUARD_BEGIN
  Json j;
  j.arr();
  uint32 limit = get_ordinal_limit(nullptr);
  for ( uint32 ord = 1; ord < limit; ord++ )
  {
    const char *name = get_numbered_type_name(nullptr, ord);
    if ( name == nullptr )
      continue;
    tinfo_t t;
    if ( !t.get_numbered_type(nullptr, ord) )
      continue;
    const char *kind = t.is_struct() ? "struct"
                     : t.is_union() ? "union"
                     : t.is_enum() ? "enum"
                     : t.is_func() ? "func"
                     : t.is_typedef() ? "typedef"
                     : "other";
    qstring decl;
    t.print(&decl, name, PRTYPE_MULTI | PRTYPE_TYPE | PRTYPE_SEMI | PRTYPE_DEF);
    size_t size = t.get_size();
    j.obj()
      .kv("ordinal", (uint64_t)ord)
      .kv("name", name)
      .kv("kind", kind)
      .key("size").opt_u64(size, BADSIZE)
      .kv("decl", qs(decl))
      .end_obj();
  }
  j.end_arr();
  return ok(j);
  GUARD_END
}

//--------------------------------------------------------------------------
int izs_listing_down(ea_t ea, uint32 count)
{
  REQUIRE_DB();
  GUARD_BEGIN
  if ( ea < inf_get_min_ea() )
    ea = inf_get_min_ea();
  ea_t head = get_item_head(ea);
  if ( head != BADADDR )
    ea = head;
  Json j;
  j.obj().key("lines").arr();
  bool at_end = false;
  ea_t next = listing_down(j, ea, count, BADADDR, &at_end);
  j.end_arr();
  j.key("next").opt_u64(next, BADADDR);
  j.kvb("at_edge", at_end);
  j.end_obj();
  return ok(j);
  GUARD_END
}

int izs_listing_up(ea_t ea, uint32 count)
{
  REQUIRE_DB();
  GUARD_BEGIN
  // Walk back item by item until enough lines precede `ea`, then generate
  // forward from there so lines come out in address order.
  ea_t min_ea = inf_get_min_ea();
  ea_t start = get_item_head(ea);
  if ( start == BADADDR || start > ea )
    start = ea;
  uint32 lines = 0;
  bool at_start = false;
  while ( lines < count )
  {
    ea_t prev = prev_not_tail(start);
    if ( prev == BADADDR || prev < min_ea )
    {
      at_start = true;
      break;
    }
    start = prev;
    idaplace_t pl(start, 0);
    linearray_t ln(nullptr);
    ln.set_place(&pl);
    lines += qmax(1, ln.get_linecnt());
  }
  Json j;
  j.obj().key("lines").arr();
  bool at_end = false;
  if ( start < ea )
    listing_down(j, start, UINT32_MAX, ea, &at_end);
  j.end_arr();
  j.key("next").opt_u64(start < ea ? start : BADADDR, BADADDR);
  j.kvb("at_edge", at_start);
  j.end_obj();
  return ok(j);
  GUARD_END
}

int izs_graph(ea_t ea)
{
  REQUIRE_DB();
  GUARD_BEGIN
  func_t *pfn = get_func(ea);
  if ( pfn == nullptr )
    return fail("no function at %a", ea);
  qflow_chart_t fc("", pfn, BADADDR, BADADDR, FC_NOEXT);
  Json j;
  j.obj()
    .kv("func", (uint64_t)pfn->start_ea)
    .kv("name", short_name(pfn->start_ea))
    .key("blocks").arr();
  for ( int i = 0; i < fc.size(); i++ )
  {
    const qbasic_block_t &bb = fc.blocks[i];
    j.obj().kv("start", (uint64_t)bb.start_ea).kv("end", (uint64_t)bb.end_ea);
    j.key("lines").arr();
    bool at_end = false;
    listing_down(j, bb.start_ea, UINT32_MAX, bb.end_ea, &at_end);
    j.end_arr();
    j.key("succs").arr();
    int ns = fc.nsucc(i);
    for ( int k = 0; k < ns; k++ )
    {
      int to = fc.succ(i, k);
      const char *kind = "always";
      if ( ns == 2 )
        kind = fc.blocks[to].start_ea == bb.end_ea ? "not_taken" : "taken";
      j.obj().kv("to", (uint64_t)to).kv("kind", kind).end_obj();
    }
    j.end_arr();
    j.kvb("is_ret", fc.is_ret_block(i));
    j.end_obj();
  }
  j.end_arr().end_obj();
  return ok(j);
  GUARD_END
}

//--------------------------------------------------------------------------
static cfuncptr_t decompile_at(ea_t ea, qstring *err, bool fresh = false)
{
  func_t *pfn = get_func(ea);
  if ( pfn == nullptr )
  {
    err->sprnt("no function at %a", ea);
    return cfuncptr_t(nullptr);
  }
  hexrays_failure_t hf;
  cfuncptr_t cf = decompile(pfn, &hf, DECOMP_NO_WAIT | (fresh ? DECOMP_NO_CACHE : 0));
  if ( cf == nullptr )
    err->sprnt("%a: %s", hf.errea, hf.desc().c_str());
  return cf;
}

// Prints the pseudocode without casts, the effect of Hex-Rays' `\` key. The
// SDK has no print option for it, so a fresh decompilation (never the cached
// object the other requests share) has its casts replaced by their operands.
struct cast_hider_t : public ctree_visitor_t
{
  cast_hider_t() : ctree_visitor_t(CV_FAST) {}
  int idaapi visit_expr(cexpr_t *e) override
  {
    while ( e->op == cot_cast && e->x != nullptr )
    {
      // The operand is a child of `e`: replace_by abandons the children and
      // deletes its argument, so hand it a copy.
      e->replace_by(new cexpr_t(*e->x));
    }
    return 0;
  }
};

int izs_decompile(ea_t ea, bool hide_casts)
{
  REQUIRE_DB();
  GUARD_BEGIN
  if ( !g_hexrays )
    return fail("the Hex-Rays decompiler is not available for this database");
  func_t *pfn = get_func(ea);
  Json j;
  j.obj();
  j.kv("func", (uint64_t)(pfn != nullptr ? pfn->start_ea : ea));
  j.kv("name", pfn != nullptr ? short_name(pfn->start_ea) : std::string());
  qstring err;
  cfuncptr_t cf = decompile_at(ea, &err, hide_casts);
  if ( cf != nullptr && hide_casts )
  {
    cast_hider_t hider;
    hider.apply_to(&cf->body, nullptr);
    cf->refresh_func_ctext();
  }
  j.key("lines").arr();
  if ( cf != nullptr )
  {
    const strvec_t &sv = cf->get_pseudocode();
    for ( const simpleline_t &sl : sv )
      j.str(qs(sl.line));
  }
  j.end_arr();
  // The addresses of the ctree items on each line (from the item anchors in
  // the tagged text), for breakpoint and instruction pointer markers.
  j.key("line_eas").arr();
  if ( cf != nullptr )
  {
    const strvec_t &sv = cf->get_pseudocode();
    for ( const simpleline_t &sl : sv )
    {
      std::vector<uint64_t> eas;
      const char *p = sl.line.c_str();
      const size_t digits = COLOR_ADDR_SIZE;
      while ( *p != '\0' )
      {
        if ( *p == COLOR_ON && p[1] == COLOR_ADDR && strlen(p + 2) >= digits )
        {
          char buf[32];
          qstrncpy(buf, p + 2, digits + 1);
          uint32 value = (uint32)strtoull(buf, nullptr, 16);
          uint32 index = value & ANCHOR_INDEX;
          if ( (value & ANCHOR_MASK) == ANCHOR_CITEM && index < cf->treeitems.size() )
          {
            ea_t at = cf->treeitems[index]->ea;
            if ( at != BADADDR && std::find(eas.begin(), eas.end(), (uint64_t)at) == eas.end() )
              eas.push_back(at);
          }
          p += 2 + digits;
        }
        else
          p++;
      }
      j.arr();
      for ( uint64_t e : eas )
        j.u64(e);
      j.end_arr();
    }
  }
  j.end_arr();
  if ( cf == nullptr )
    j.kv("error", qs(err));
  else
    j.key("error").null();
  j.end_obj();
  return ok(j);
  GUARD_END
}

static const lvar_t *find_lvar(cfunc_t *cf, const char *name)
{
  lvars_t *lv = cf->get_lvars();
  if ( lv == nullptr )
    return nullptr;
  for ( const lvar_t &v : *lv )
    if ( v.name == name )
      return &v;
  return nullptr;
}

static void emit_lvar(Json &j, const lvar_t &v)
{
  qstring type;
  v.type().print(&type);
  j.kv("lvar", qs(v.name)).kv("lvar_type", qs(type));
}

int izs_pseudo_item(ea_t func, uint64 anchor)
{
  REQUIRE_DB();
  GUARD_BEGIN
  if ( !g_hexrays )
    return fail("the Hex-Rays decompiler is not available");
  qstring err;
  cfuncptr_t cf = decompile_at(func, &err);
  if ( cf == nullptr )
    return fail("%s", err.c_str());
  cf->get_pseudocode(); // fills treeitems
  lvars_t *lv = cf->get_lvars();
  Json j;
  j.obj();
  uint32 value = (uint32)anchor;
  uint32 index = value & ANCHOR_INDEX;
  if ( (value & ANCHOR_MASK) == ANCHOR_LVAR )
  {
    j.key("ea").null().kv("op", "lvar_decl").key("target").null();
    if ( lv != nullptr && index < lv->size() )
      emit_lvar(j, (*lv)[index]);
    else
      j.key("lvar").null().key("lvar_type").null();
    j.end_obj();
    return ok(j);
  }
  if ( (value & ANCHOR_MASK) != ANCHOR_CITEM || index >= cf->treeitems.size() )
  {
    j.key("ea").null().kv("op", "none").key("target").null()
     .key("lvar").null().key("lvar_type").null();
    j.end_obj();
    return ok(j);
  }
  const citem_t *it = cf->treeitems[index];
  j.key("ea").opt_u64(it->ea, BADADDR);
  const char *op = get_ctype_name(it->op);
  j.kv("op", op != nullptr ? op : "?");
  ea_t target = BADADDR;
  const lvar_t *var = nullptr;
  if ( it->is_expr() )
  {
    const cexpr_t *e = (const cexpr_t *)it;
    if ( e->op == cot_obj )
      target = e->obj_ea;
    else if ( e->op == cot_call && e->x != nullptr && e->x->op == cot_obj )
      target = e->x->obj_ea;
    else if ( e->op == cot_var && lv != nullptr && e->v.idx < lv->size() )
      var = &(*lv)[e->v.idx];
  }
  j.key("target").opt_u64(target, BADADDR);
  if ( var != nullptr )
    emit_lvar(j, *var);
  else
    j.key("lvar").null().key("lvar_type").null();
  j.end_obj();
  return ok(j);
  GUARD_END
}

int izs_rename_lvar(ea_t func, const char *oldname, const char *newname)
{
  REQUIRE_DB();
  GUARD_BEGIN
  if ( !g_hexrays )
    return fail("the Hex-Rays decompiler is not available");
  undo_point();
  if ( !rename_lvar(func, oldname, newname) )
    return fail("cannot rename %s to %s", oldname, newname);
  mark_cfunc_dirty(func);
  return ok_null();
  GUARD_END
}

int izs_set_lvar_type(ea_t func, const char *name, const char *decl)
{
  REQUIRE_DB();
  GUARD_BEGIN
  if ( !g_hexrays )
    return fail("the Hex-Rays decompiler is not available");
  qstring err;
  cfuncptr_t cf = decompile_at(func, &err);
  if ( cf == nullptr )
    return fail("%s", err.c_str());
  const lvar_t *v = find_lvar(cf, name);
  if ( v == nullptr )
    return fail("no local variable named %s", name);
  qstring text;
  text.sprnt("%s __izs_tmp;", decl);
  tinfo_t tif;
  qstring parsed_name;
  if ( !parse_decl(&tif, &parsed_name, nullptr, text.c_str(), PT_SIL) )
    return fail("cannot parse type: %s", decl);
  lvar_saved_info_t info;
  info.ll = *v;
  info.type = tif;
  undo_point();
  if ( !modify_user_lvar_info(func, MLI_TYPE, info) )
    return fail("cannot set the type of %s", name);
  mark_cfunc_dirty(func);
  return ok_null();
  GUARD_END
}

//--------------------------------------------------------------------------
int izs_addr_info(ea_t ea)
{
  REQUIRE_DB();
  GUARD_BEGIN
  Json j;
  j.obj().kv("ea", (uint64_t)ea);
  ea_t head = get_item_head(ea);
  j.kv("head", (uint64_t)head);
  segment_t *s = getseg(ea);
  if ( s != nullptr )
  {
    qstring sn;
    get_visible_segm_name(&sn, s);
    j.kv("segment", qs(sn));
  }
  else
    j.key("segment").null();
  func_t *pfn = get_func(ea);
  if ( pfn != nullptr )
    j.kv("function", short_name(pfn->start_ea)).kv("func_start", (uint64_t)pfn->start_ea);
  else
    j.key("function").null().key("func_start").null();
  qstring nm = get_short_name(head);
  if ( nm.empty() )
    j.key("name").null();
  else
    j.kv("name", qs(nm));
  flags64_t f = get_flags(ea);
  const char *kind = !is_mapped(ea) ? "unmapped"
                   : is_code(f) ? "code"
                   : is_strlit(f) ? "string"
                   : is_data(f) ? "data"
                   : is_tail(f) ? "tail"
                   : "unknown";
  j.kv("kind", kind);
  qoff64_t off = get_fileregion_offset(ea);
  j.key("file_offset").opt_u64((uint64_t)off, (uint64_t)-1);
  j.end_obj();
  return ok(j);
  GUARD_END
}

int izs_resolve(const char *expr, ea_t from)
{
  REQUIRE_DB();
  GUARD_BEGIN
  ea_t ea = BADADDR;
  if ( !str2ea(&ea, expr, from) )
    ea = get_name_ea(BADADDR, expr);
  Json j;
  j.opt_u64(ea, BADADDR);
  return ok(j);
  GUARD_END
}

void emit_xref(Json &j, ea_t from, ea_t to, uchar type, ea_t text_at)
{
  char kind[2] = { xrefchar(type), '\0' };
  j.obj().kv("from", (uint64_t)from).kv("to", (uint64_t)to).kv("kind", kind);
  func_t *pfn = get_func(text_at);
  if ( pfn != nullptr )
  {
    qstring fn;
    fn.sprnt("%s+%llX", short_name(pfn->start_ea).c_str(),
             (unsigned long long)(text_at - pfn->start_ea));
    j.kv("func", qs(fn));
  }
  else
    j.key("func").null();
  qstring line;
  generate_disasm_line(&line, text_at, GENDSM_REMOVE_TAGS);
  j.kv("text", qs(line));
  j.end_obj();
}

int izs_xrefs_to(ea_t ea)
{
  REQUIRE_DB();
  GUARD_BEGIN
  Json j;
  j.arr();
  xrefblk_t xb;
  for ( bool more = xb.first_to(ea, XREF_ALL); more; more = xb.next_to() )
  {
    if ( xb.type == fl_F )
      continue; // ordinary flow is noise in an xref list
    emit_xref(j, xb.from, ea, xb.type, xb.from);
  }
  j.end_arr();
  return ok(j);
  GUARD_END
}

int izs_xrefs_from(ea_t ea)
{
  REQUIRE_DB();
  GUARD_BEGIN
  Json j;
  j.arr();
  // Gather from every address of the item (operands may reference anywhere).
  ea_t head = get_item_head(ea);
  ea_t end = get_item_end(head);
  for ( ea_t a = head; a < end; a++ )
  {
    xrefblk_t xb;
    for ( bool more = xb.first_from(a, XREF_ALL); more; more = xb.next_from() )
    {
      if ( xb.type == fl_F )
        continue;
      emit_xref(j, a, xb.to, xb.type, xb.to);
    }
  }
  j.end_arr();
  return ok(j);
  GUARD_END
}

int izs_bytes(ea_t ea, uint32 len)
{
  REQUIRE_DB();
  GUARD_BEGIN
  static const char hex[] = "0123456789ABCDEF";
  std::string out;
  out.reserve(len * 2);
  for ( uint32 i = 0; i < len; i++ )
  {
    ea_t a = ea + i;
    if ( !is_loaded(a) )
    {
      out += "??";
      continue;
    }
    uchar b = get_db_byte(a);
    out += hex[b >> 4];
    out += hex[b & 15];
  }
  Json j;
  j.obj().kv("ea", (uint64_t)ea).kv("hex", out).end_obj();
  return ok(j);
  GUARD_END
}

int izs_find_bytes(ea_t ea, const char *pattern, bool forward)
{
  REQUIRE_DB();
  GUARD_BEGIN
  compiled_binpat_vec_t bv;
  qstring err;
  if ( !parse_binpat_str(&bv, ea, pattern, 16, PBSENC_DEF1BPU, &err) )
    return fail("bad pattern: %s", err.c_str());
  ea_t found;
  if ( forward )
    found = bin_search(ea + 1, inf_get_max_ea(), bv, BIN_SEARCH_FORWARD | BIN_SEARCH_NOSHOW);
  else
    found = bin_search(inf_get_min_ea(), ea, bv, BIN_SEARCH_BACKWARD | BIN_SEARCH_NOSHOW);
  Json j;
  j.opt_u64(found, BADADDR);
  return ok(j);
  GUARD_END
}

int izs_find_text(ea_t ea, const char *text, bool forward)
{
  REQUIRE_DB();
  GUARD_BEGIN
  int flags = SEARCH_NEXT | SEARCH_NOSHOW | (forward ? SEARCH_DOWN : SEARCH_UP);
  ea_t found = find_text(ea, 0, 0, text, flags);
  Json j;
  j.opt_u64(found, BADADDR);
  return ok(j);
  GUARD_END
}

//--------------------------------------------------------------------------
// Mutations. Each records an undo point first.

int izs_rename(ea_t ea, const char *name)
{
  REQUIRE_DB();
  GUARD_BEGIN
  undo_point();
  if ( !set_name(ea, name, SN_NOWARN | SN_NOCHECK | SN_FORCE) )
    return fail("cannot rename %a to '%s'", ea, name);
  return ok_null();
  GUARD_END
}

int izs_set_comment(ea_t ea, const char *text, bool rpt)
{
  REQUIRE_DB();
  GUARD_BEGIN
  undo_point();
  if ( !set_cmt(ea, text, rpt) )
    return fail("cannot set the comment at %a", ea);
  return ok_null();
  GUARD_END
}

int izs_set_func_comment(ea_t ea, const char *text, bool rpt)
{
  REQUIRE_DB();
  GUARD_BEGIN
  func_t *pfn = get_func(ea);
  if ( pfn == nullptr )
    return fail("no function at %a", ea);
  undo_point();
  if ( !set_func_cmt(pfn, text, rpt) )
    return fail("cannot set the function comment");
  return ok_null();
  GUARD_END
}

int izs_get_comment(ea_t ea, bool rpt)
{
  REQUIRE_DB();
  GUARD_BEGIN
  qstring c;
  get_cmt(&c, ea, rpt);
  Json j;
  j.str(qs(c));
  return ok(j);
  GUARD_END
}

int izs_get_type(ea_t ea)
{
  REQUIRE_DB();
  GUARD_BEGIN
  qstring out;
  if ( !print_type(&out, ea, PRTYPE_1LINE | PRTYPE_SEMI) )
  {
    tinfo_t tif;
    if ( guess_tinfo(&tif, ea) != GUESS_FUNC_FAILED )
    {
      qstring name = get_name(ea);
      tif.print(&out, name.c_str(), PRTYPE_1LINE | PRTYPE_SEMI);
    }
  }
  Json j;
  j.str(qs(out));
  return ok(j);
  GUARD_END
}

int izs_set_type(ea_t ea, const char *decl)
{
  REQUIRE_DB();
  GUARD_BEGIN
  undo_point();
  if ( decl[0] == '\0' )
  {
    del_tinfo(ea);
    return ok_null();
  }
  if ( !apply_cdecl(nullptr, ea, decl, TINFO_DEFINITE) )
    return fail("cannot apply type: %s", decl);
  if ( g_hexrays )
    mark_cfunc_dirty(ea);
  return ok_null();
  GUARD_END
}

int izs_set_local_type(const char *decl)
{
  REQUIRE_DB();
  GUARD_BEGIN
  tinfo_t tif;
  qstring name;
  if ( !parse_decl(&tif, &name, nullptr, decl, PT_SIL) || name.empty() )
    return fail("cannot parse declaration");
  undo_point();
  tinfo_code_t rc = tif.set_named_type(nullptr, name.c_str(), NTF_REPLACE);
  if ( rc != TERR_OK )
    return fail("cannot save type %s: %s", name.c_str(), tinfo_errstr(rc));
  return ok_null();
  GUARD_END
}

int izs_make_code(ea_t ea)
{
  REQUIRE_DB();
  GUARD_BEGIN
  undo_point();
  if ( create_insn(ea) == 0 )
    return fail("cannot create an instruction at %a", ea);
  ev_run_auto(false);
  return ok_null();
  GUARD_END
}

int izs_make_data(ea_t ea)
{
  REQUIRE_DB();
  GUARD_BEGIN
  undo_point();
  flags64_t f = get_flags(ea);
  bool done;
  if ( is_byte(f) )
    done = create_word(ea, 2);
  else if ( is_word(f) )
    done = create_dword(ea, 4);
  else if ( is_dword(f) )
    done = create_qword(ea, 8);
  else
  {
    del_items(ea, DELIT_SIMPLE, 1);
    done = create_byte(ea, 1);
  }
  if ( !done )
  {
    del_items(ea, DELIT_SIMPLE, 1);
    done = create_byte(ea, 1);
  }
  if ( !done )
    return fail("cannot create data at %a", ea);
  return ok_null();
  GUARD_END
}

int izs_make_string(ea_t ea)
{
  REQUIRE_DB();
  GUARD_BEGIN
  undo_point();
  if ( !create_strlit(ea, 0, STRTYPE_C) )
    return fail("cannot create a string at %a", ea);
  return ok_null();
  GUARD_END
}

int izs_undefine(ea_t ea)
{
  REQUIRE_DB();
  GUARD_BEGIN
  undo_point();
  if ( !del_items(ea, DELIT_SIMPLE, 1) )
    return fail("cannot undefine %a", ea);
  return ok_null();
  GUARD_END
}

int izs_make_function(ea_t ea)
{
  REQUIRE_DB();
  GUARD_BEGIN
  undo_point();
  if ( !add_func(ea) )
    return fail("cannot create a function at %a", ea);
  ev_run_auto(false);
  return ok_null();
  GUARD_END
}

int izs_delete_function(ea_t ea)
{
  REQUIRE_DB();
  GUARD_BEGIN
  undo_point();
  if ( !del_func(ea) )
    return fail("no function to delete at %a", ea);
  return ok_null();
  GUARD_END
}

int izs_toggle_collapse(ea_t ea)
{
  REQUIRE_DB();
  GUARD_BEGIN
  func_t *pfn = get_func(ea);
  if ( pfn != nullptr && pfn->start_ea == get_item_head(ea) )
  {
    pfn->flags ^= FUNC_HIDDEN;
    if ( !update_func(pfn) )
      return fail("cannot update the function at %a", ea);
    return ok_null();
  }
  hidden_range_t *hr = get_hidden_range(ea);
  if ( hr != nullptr )
  {
    hr->visible = !hr->visible;
    if ( !update_hidden_range(hr) )
      return fail("cannot update the hidden range at %a", ea);
    return ok_null();
  }
  if ( pfn != nullptr )
  {
    pfn->flags ^= FUNC_HIDDEN;
    update_func(pfn);
    return ok_null();
  }
  return fail("nothing to collapse at %a", ea);
  GUARD_END
}

int izs_undo(void)
{
  REQUIRE_DB();
  GUARD_BEGIN
  Json j;
  j.boolean(perform_undo());
  return ok(j);
  GUARD_END
}

int izs_redo(void)
{
  REQUIRE_DB();
  GUARD_BEGIN
  Json j;
  j.boolean(perform_redo());
  return ok(j);
  GUARD_END
}

// Search > Next code / data / unexplored / not function: the first address
// of that kind after (or before) `ea`.
int izs_find_kind(ea_t ea, const char *kind, bool forward)
{
  REQUIRE_DB();
  GUARD_BEGIN
  int flags = SEARCH_NEXT | SEARCH_NOSHOW | SEARCH_NOBRK | (forward ? SEARCH_DOWN : SEARCH_UP);
  std::string k = kind;
  ea_t found;
  if ( k == "code" )
    found = find_code(ea, flags);
  else if ( k == "data" )
    found = find_data(ea, flags);
  else if ( k == "unknown" )
    found = find_unknown(ea, flags);
  else if ( k == "notfunc" )
    found = find_not_func(ea, flags);
  else
    return fail("unknown search kind '%s'", kind);
  Json j;
  j.opt_u64(found, BADADDR);
  return ok(j);
  GUARD_END
}

// Search > Immediate value: the next instruction with `value` as an operand.
int izs_find_imm(ea_t ea, uint64_t value, bool forward)
{
  REQUIRE_DB();
  GUARD_BEGIN
  int flags = SEARCH_NEXT | SEARCH_NOSHOW | SEARCH_NOBRK | (forward ? SEARCH_DOWN : SEARCH_UP);
  int opnum = 0;
  ea_t found = find_imm(ea, flags, (uval_t)value, &opnum);
  Json j;
  j.opt_u64(found, BADADDR);
  return ok(j);
  GUARD_END
}

} // extern "C"
