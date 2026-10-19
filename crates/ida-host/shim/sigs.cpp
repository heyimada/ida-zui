// FLIRT signatures and Lumina. FLIRT lists come from the kernel's own
// signature table (planned / applied) and from the .sig files on disk;
// Lumina has no SDK API, so its actions are the kernel's UI actions run
// through process_ui_action and judged by the functions they changed.

#include <pro.h>
#include <ida.hpp>
#include <auto.hpp>
#include <diskio.hpp>
#include <funcs.hpp>
#include <kernwin.hpp>
#include <loader.hpp>
#include <registry.hpp>

#include <algorithm>
#include <cstdlib>
#include <string>
#include <vector>

#include "common.hpp"
#include "events.hpp"

static uint64_t count_funcs(uint32 flag)
{
  uint64_t n = 0;
  size_t qty = get_func_qty();
  for ( size_t i = 0; i < qty; i++ )
  {
    func_t *f = getn_func(i);
    if ( f != nullptr && (f->flags & flag) != 0 )
      n++;
  }
  return n;
}

//--------------------------------------------------------------------------
// FLIRT

// Title and function count from the header of a .sig file (the format the
// FLIRT tools write: magic, version, then fields that grew with the version).
static bool read_sig_header(const std::string &path, std::string *title, uint64_t *nfuncs)
{
  linput_t *li = open_linput(path.c_str(), false);
  if ( li == nullptr )
    return false;
  uchar h[96] = {};
  ssize_t read = qlread(li, h, sizeof(h));
  close_linput(li);
  size_t got = read > 0 ? (size_t)read : 0;
  if ( got < 48 || memcmp(h, "IDASGN", 6) != 0 )
    return false;
  int ver = h[6];
  size_t off = 37;
  *nfuncs = (uint64_t)h[18] | ((uint64_t)h[19] << 8);
  if ( ver >= 6 )
  {
    *nfuncs = (uint64_t)h[37] | ((uint64_t)h[38] << 8) | ((uint64_t)h[39] << 16) | ((uint64_t)h[40] << 24);
    off += 4;
  }
  if ( ver >= 8 )
    off += 2;
  if ( ver >= 10 )
    off += 2;
  size_t len = h[34];
  if ( off + len > got )
    return false;
  title->assign((const char *)h + off, len);
  return true;
}

struct sig_files_t : public file_enumerator_t
{
  std::vector<std::string> paths;
  int visit_file(const char *file) override
  {
    paths.push_back(file);
    return 0;
  }
};

static std::string base_name(const std::string &path)
{
  size_t s = path.find_last_of("/\\");
  std::string n = s == std::string::npos ? path : path.substr(s + 1);
  size_t dot = n.rfind('.');
  return dot == std::string::npos ? n : n.substr(0, dot);
}

static const char *state_name(int st)
{
  switch ( st )
  {
    case IDASGN_APPLIED: return "applied";
    case IDASGN_CURRENT: return "current";
    case IDASGN_PLANNED: return "planned";
    default:             return "unknown";
  }
}

// {procname, lib_functions, applied:[...], available:[...]}
static void emit_sig_state(Json &j)
{
  qstring proc = inf_get_procname();
  j.obj().kv("processor", qs(proc)).kv("lib_functions", count_funcs(FUNC_LIB));
  std::vector<std::string> applied_names;
  j.key("applied").arr();
  int qty = get_idasgn_qty();
  for ( int i = 0; i < qty; i++ )
  {
    qstring name, opt, title;
    int matched = get_idasgn_desc(&name, &opt, i);
    std::string nm = qs(name);
    // Names come back with or without the extension; keep the bare name.
    size_t dot = nm.rfind(".sig");
    if ( dot != std::string::npos && dot + 4 == nm.size() )
      nm.resize(dot);
    applied_names.push_back(nm);
    get_idasgn_title(&title, nm.c_str());
    j.obj()
      .kv("index", (uint64_t)i)
      .kv("name", nm)
      .kv("title", qs(title))
      .kv("optlibs", qs(opt))
      .kv("state", state_name(calc_idasgn_state(i)))
      .kvi("matched", matched)
      .end_obj();
  }
  j.end_arr();

  // Every .sig file the kernel would find for this processor.
  qstrvec_t dirs;
  get_ida_subdirs(&dirs, SIG_SUBDIR, IDA_SUBDIR_IDP | IDA_SUBDIR_ONLY_EXISTING);
  std::vector<std::string> seen;
  j.key("available").arr();
  for ( const qstring &dir : dirs )
  {
    sig_files_t files;
    char buf[QMAXPATH];
    enumerate_files(buf, sizeof(buf), dir.c_str(), "*.sig", files);
    std::sort(files.paths.begin(), files.paths.end());
    for ( const std::string &path : files.paths )
    {
      std::string name = base_name(path);
      if ( std::find(seen.begin(), seen.end(), name) != seen.end() )
        continue;
      seen.push_back(name);
      std::string title;
      uint64_t nfuncs = 0;
      read_sig_header(path, &title, &nfuncs);
      j.obj()
        .kv("name", name)
        .kv("title", title)
        .kv("path", path)
        .kv("functions", nfuncs)
        .kvb("applied", std::find(applied_names.begin(), applied_names.end(), name) != applied_names.end())
        .end_obj();
    }
  }
  j.end_arr();
  j.end_obj();
}

extern "C" int izs_sig_list(void)
{
  REQUIRE_DB();
  GUARD_BEGIN
  Json j;
  emit_sig_state(j);
  return ok(j);
  GUARD_END
}

// `names` is one signature name per line. The signatures are planned and the
// analysis queue then runs over the whole database (reporting progress).
extern "C" int izs_sig_apply(const char *names)
{
  REQUIRE_DB();
  GUARD_BEGIN
  std::vector<std::string> list;
  std::string cur;
  for ( const char *p = names; ; p++ )
  {
    if ( *p == '\n' || *p == '\0' )
    {
      if ( !cur.empty() )
        list.push_back(cur);
      cur.clear();
      if ( *p == '\0' )
        break;
    }
    else if ( *p != '\r' )
    {
      cur += *p;
    }
  }
  if ( list.empty() )
    return fail("no signature given");
  undo_point();
  uint64_t lib_before = count_funcs(FUNC_LIB);
  Json failed;
  failed.arr();
  uint64_t planned = 0;
  for ( const std::string &n : list )
  {
    if ( plan_to_apply_idasgn(n.c_str()) != 0 )
      planned++;
    else
      failed.str(n);
  }
  failed.end_arr();
  if ( planned != 0 )
  {
    // Planning only lists the file; the library-function queue then has to
    // look at every function again.
    plan_range(inf_get_min_ea(), inf_get_max_ea());
    ev_run_auto(true);
  }
  Json j;
  j.obj()
    .kv("planned", planned)
    .key("failed").raw(failed.s)
    .kv("lib_before", lib_before)
    .kv("lib_after", count_funcs(FUNC_LIB))
    .key("state");
  emit_sig_state(j);
  j.end_obj();
  return ok(j);
  GUARD_END
}

//--------------------------------------------------------------------------
// Lumina

static bool push_blocked()
{
  return qgetenv("IZS_BLOCK_LUMINA_PUSH", nullptr) || qgetenv("IDA_ZUI_AUTOMATION", nullptr);
}

extern "C" int izs_lumina_status(void)
{
  GUARD_BEGIN
  qstring primary, secondary;
  reg_read_string(&primary, "Primary", "Lumina");
  reg_read_string(&secondary, "Secondary", "Lumina");
  Json j;
  j.obj()
    .kv("server", qs(primary))
    .kv("secondary", qs(secondary))
    .kvb("auto_use", reg_read_int("AutoUseLumina", 0) != 0)
    .kv("functions", g_open ? count_funcs(FUNC_LUMINA) : 0)
    .kvb("push_blocked", push_blocked())
    .end_obj();
  return ok(j);
  GUARD_END
}

// action: 0 pull all, 1 pull the function at `ea`, 2 push all, 3 push that
// function. A push uploads the user's analysis to the Lumina server, so it
// runs only when the caller says it was confirmed and never under automation.
extern "C" int izs_lumina(int action, uint64_t ea, bool confirmed)
{
  REQUIRE_DB();
  GUARD_BEGIN
  static const char *const names[] = { "LuminaPullAllMds", "LuminaPullMd", "LuminaPushAllMds", "LuminaPushMd" };
  if ( action < 0 || action > 3 )
    return fail("unknown Lumina action %d", action);
  bool push = action >= 2;
  if ( push && !confirmed )
    return fail("Lumina push refused: it uploads your analysis to the server and needs explicit confirmation");
  if ( push && push_blocked() )
    return fail("Lumina push refused: pushes are blocked in this environment (automation or IZS_BLOCK_LUMINA_PUSH)");
  if ( (action & 1) != 0 && get_func((ea_t)ea) == nullptr )
    return fail("the address is not inside a function");
  uint64_t before = count_funcs(FUNC_LUMINA);
  uint64_t total = get_func_qty();
  ev_set_screen_ea((ea_t)ea);
  ev_capture_begin();
  undo_point();
  bool dispatched = process_ui_action(names[action]);
  // Lumina works in the background of the analysis queue; let it finish.
  ev_run_auto(false);
  std::vector<std::string> log = ev_capture_end();
  Json j;
  j.obj()
    .kv("action", names[action])
    .kvb("dispatched", dispatched)
    .kv("functions", total)
    .kv("lumina_before", before)
    .kv("lumina_after", count_funcs(FUNC_LUMINA))
    .key("log").arr();
  for ( const std::string &l : log )
    j.str(l);
  j.end_arr().end_obj();
  return ok(j);
  GUARD_END
}
