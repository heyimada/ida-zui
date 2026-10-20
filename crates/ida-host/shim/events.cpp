// Host events. Three sources feed the UI between (and during) requests:
//   * ui_msg notifications: everything IDA, plugins and Python print;
//   * IDB notifications: renames, comments, functions, types, patches and so
//     on, coalesced into one event per ~120 ms with an address range, so a
//     script that renames 10k names produces a handful of events;
//   * the auto-analysis loop: stage, address and a monotonic percentage.
// Hooks run on the idalib thread; a small flusher thread makes sure pending
// events do not wait for the end of a long request.

#include <pro.h>
#include <ida.hpp>
#include <idp.hpp>
#include <auto.hpp>
#include <funcs.hpp>
#include <kernwin.hpp>
#include <segment.hpp>
#include <ua.hpp>

#include <atomic>
#include <chrono>
#include <condition_variable>
#include <mutex>
#include <thread>
#include <vector>

#include "common.hpp"
#include "events.hpp"

using Clock = std::chrono::steady_clock;

static ev_emit_fn g_emit = nullptr;
static std::mutex g_mu;
static std::atomic<int> g_origin{EV_REQUEST};
static std::atomic<ea_t> g_screen_ea{BADADDR};

// ---- log lines ---------------------------------------------------------

static std::vector<std::string> g_lines;
static std::string g_partial;
// Lines IDA printed while a request that wants to quote them is running.
static bool g_capturing = false;
static std::vector<std::string> g_captured;

// ---- database changes ----------------------------------------------------

enum Kind : uint32_t
{
  K_NAME = 1 << 0,
  K_CMT = 1 << 1,
  K_FUNC = 1 << 2,
  K_TYPE = 1 << 3,
  K_CODE = 1 << 4,
  K_DATA = 1 << 5,
  K_PATCH = 1 << 6,
  K_SEG = 1 << 7,
  K_LTYPE = 1 << 8,
  K_FRAME = 1 << 9,
  K_DISPLAY = 1 << 10,
};

static const char *const kind_names[] = {
  "name", "cmt", "func", "type", "code", "data", "patch", "seg", "ltype", "frame", "display",
};

struct Pending
{
  bool any = false;
  bool global = false;
  int origin = EV_REQUEST;
  uint32_t kinds = 0;
  uint64_t count = 0;
  ea_t lo = BADADDR;
  ea_t hi = 0;
  bool has_range = false;
};

static Pending g_pending;
static bool g_py_changed = false;
static Clock::time_point g_last_flush = Clock::now();
static std::atomic<bool> g_dirty{false};

void ev_set_emitter(ev_emit_fn fn)
{
  g_emit = fn;
}

static void emit_locked(const std::string &json)
{
  if ( g_emit != nullptr )
    g_emit(json.data(), json.size());
}

void ev_emit(const std::string &json)
{
  std::lock_guard<std::mutex> lk(g_mu);
  emit_locked(json);
}

static void flush_log_locked()
{
  if ( !g_partial.empty() )
  {
    g_lines.push_back(g_partial);
    g_partial.clear();
  }
  if ( g_lines.empty() )
    return;
  Json j;
  j.obj().kv("event", "log").key("lines").arr();
  for ( const std::string &l : g_lines )
    j.str(l);
  j.end_arr().end_obj();
  g_lines.clear();
  emit_locked(j.s);
}

static void flush_db_locked()
{
  if ( !g_pending.any )
    return;
  Json j;
  j.obj()
    .kv("event", "db_changed")
    .kv("origin", g_pending.origin == EV_PYTHON ? "python" : "request")
    .key("kinds")
    .arr();
  for ( size_t i = 0; i < qnumber(kind_names); i++ )
    if ( g_pending.kinds & (1u << i) )
      j.str(kind_names[i]);
  j.end_arr();
  j.kvb("global", g_pending.global || !g_pending.has_range);
  j.key("lo").opt_u64(g_pending.has_range ? g_pending.lo : BADADDR, BADADDR);
  j.key("hi").opt_u64(g_pending.has_range ? g_pending.hi : BADADDR, BADADDR);
  j.kv("count", g_pending.count);
  j.end_obj();
  g_pending = Pending();
  emit_locked(j.s);
}

static void flush_all_locked()
{
  flush_log_locked();
  flush_db_locked();
  g_last_flush = Clock::now();
  g_dirty = false;
}

static void note_change(uint32_t kind, ea_t lo, ea_t hi, bool global)
{
  std::lock_guard<std::mutex> lk(g_mu);
  int origin = g_origin.load();
  if ( g_pending.any && g_pending.origin != origin )
    flush_db_locked();
  g_pending.any = true;
  g_pending.origin = origin;
  g_pending.kinds |= kind;
  g_pending.count++;
  if ( global )
    g_pending.global = true;
  else if ( lo != BADADDR )
  {
    g_pending.has_range = true;
    if ( lo < g_pending.lo )
      g_pending.lo = lo;
    if ( hi > g_pending.hi )
      g_pending.hi = hi;
  }
  if ( origin == EV_PYTHON )
    g_py_changed = true;
  g_dirty = true;
}

static void note_ea(uint32_t kind, ea_t ea, asize_t len = 1)
{
  note_change(kind, ea, ea + (len == 0 ? 1 : len), false);
}

// ---- listeners -------------------------------------------------------------

struct ui_listener_t : public event_listener_t
{
  ssize_t idaapi on_event(ssize_t code, va_list va) override
  {
    if ( code == ui_msg )
    {
      const char *fmt = va_arg(va, const char *);
      va_list args = va_arg(va, va_list);
      if ( fmt == nullptr )
        return 0;
      qstring text;
      text.vsprnt(fmt, args);
      std::lock_guard<std::mutex> lk(g_mu);
      g_partial.append(text.c_str(), text.length());
      size_t nl;
      while ( (nl = g_partial.find('\n')) != std::string::npos )
      {
        if ( g_capturing )
          g_captured.push_back(g_partial.substr(0, nl));
        g_lines.push_back(g_partial.substr(0, nl));
        g_partial.erase(0, nl + 1);
      }
      g_dirty = true;
      return 0;
    }
    if ( code == ui_screenea )
    {
      ea_t *out = va_arg(va, ea_t *);
      if ( out != nullptr )
        *out = g_screen_ea.load();
      return 1;
    }
    return 0;
  }
};

struct idb_listener_t : public event_listener_t
{
  ssize_t idaapi on_event(ssize_t code, va_list va) override
  {
    using namespace idb_event;
    switch ( code )
    {
      case renamed:
        note_ea(K_NAME, va_arg(va, ea_t));
        break;
      case cmt_changed:
      case extra_cmt_changed:
        note_ea(K_CMT, va_arg(va, ea_t));
        break;
      case func_added:
      case func_updated:
      case thunk_func_created:
      case func_noret_changed:
      {
        func_t *pfn = va_arg(va, func_t *);
        if ( pfn != nullptr )
          note_change(K_FUNC, pfn->start_ea, pfn->end_ea, false);
        break;
      }
      case func_deleted:
        note_ea(K_FUNC, va_arg(va, ea_t));
        break;
      case ti_changed:
      case op_ti_changed:
      case op_type_changed:
        note_ea(K_TYPE, va_arg(va, ea_t));
        break;
      case byte_patched:
        note_ea(K_PATCH, va_arg(va, ea_t));
        break;
      case make_code:
      {
        const insn_t *insn = va_arg(va, const insn_t *);
        if ( insn != nullptr )
          note_ea(K_CODE, insn->ea, insn->size);
        break;
      }
      case make_data:
      {
        ea_t ea = va_arg(va, ea_t);
        (void)va_arg(va, flags64_t);
        (void)va_arg(va, tid_t);
        asize_t len = va_arg(va, asize_t);
        note_ea(K_DATA, ea, len);
        break;
      }
      case destroyed_items:
      {
        ea_t a = va_arg(va, ea_t);
        ea_t b = va_arg(va, ea_t);
        note_change(K_CODE | K_DATA, a, b, false);
        break;
      }
      case item_color_changed:
      case callee_addr_changed:
        note_ea(K_DISPLAY, va_arg(va, ea_t));
        break;
      case segm_added:
      case segm_deleted:
      case segm_moved:
      case allsegs_moved:
      case segm_name_changed:
      case segm_class_changed:
      case segm_start_changed:
      case segm_end_changed:
      case segm_attrs_updated:
        note_change(K_SEG, BADADDR, BADADDR, true);
        break;
      case local_types_changed:
      case lt_udm_created:
      case lt_udm_deleted:
      case lt_udm_renamed:
      case lt_udm_changed:
      case lt_udt_expanded:
      case lt_edm_created:
      case lt_edm_deleted:
      case lt_edm_renamed:
      case lt_edm_changed:
        note_change(K_LTYPE, BADADDR, BADADDR, true);
        break;
      case frame_created:
      case frame_deleted:
      case frame_udm_created:
      case frame_udm_deleted:
      case frame_udm_renamed:
      case frame_udm_changed:
      case frame_expanded:
        note_change(K_FRAME, BADADDR, BADADDR, true);
        break;
      default:
        break;
    }
    return 0;
  }
};

static ui_listener_t g_ui_listener;
static idb_listener_t g_idb_listener;
static bool g_idb_hooked = false;

// Flushes pending events once they are ~120 ms old, whatever the main thread
// is busy with.
static void flusher()
{
  for ( ;; )
  {
    std::this_thread::sleep_for(std::chrono::milliseconds(60));
    if ( !g_dirty.load() )
      continue;
    std::lock_guard<std::mutex> lk(g_mu);
    if ( Clock::now() - g_last_flush >= std::chrono::milliseconds(120) )
      flush_all_locked();
  }
}

void ev_init()
{
  hook_event_listener(HT_UI, &g_ui_listener, nullptr);
  std::thread(flusher).detach();
}

void ev_hook_idb()
{
  if ( !g_idb_hooked )
    g_idb_hooked = hook_event_listener(HT_IDB, &g_idb_listener, nullptr);
}

void ev_unhook_idb()
{
  if ( g_idb_hooked )
  {
    unhook_event_listener(HT_IDB, &g_idb_listener);
    g_idb_hooked = false;
  }
}

void ev_begin_request(int origin)
{
  std::lock_guard<std::mutex> lk(g_mu);
  if ( g_pending.any && g_pending.origin != origin )
    flush_db_locked();
  g_origin = origin;
}

void ev_end_request()
{
  std::lock_guard<std::mutex> lk(g_mu);
  flush_all_locked();
}

void ev_capture_begin()
{
  std::lock_guard<std::mutex> lk(g_mu);
  g_capturing = true;
  g_captured.clear();
}

std::vector<std::string> ev_capture_end()
{
  std::lock_guard<std::mutex> lk(g_mu);
  if ( !g_partial.empty() )
    g_captured.push_back(g_partial);
  g_capturing = false;
  return std::move(g_captured);
}

void ev_python_begin()
{
  std::lock_guard<std::mutex> lk(g_mu);
  g_py_changed = false;
}

bool ev_python_changed()
{
  std::lock_guard<std::mutex> lk(g_mu);
  return g_py_changed;
}

void ev_set_screen_ea(ea_t ea)
{
  g_screen_ea = ea;
}

// ---- auto-analysis ------------------------------------------------------

static const char *stage_name(atype_t t)
{
  switch ( t )
  {
    case AU_UNK: return "AU_UNK";
    case AU_CODE: return "AU_CODE";
    case AU_WEAK: return "AU_WEAK";
    case AU_PROC: return "AU_PROC";
    case AU_TAIL: return "AU_TAIL";
    case AU_FCHUNK: return "AU_FCHUNK";
    case AU_USED: return "AU_USED";
    case AU_USD2: return "AU_USD2";
    case AU_TYPE: return "AU_TYPE";
    case AU_LIBF: return "AU_LIBF";
    case AU_LBF2: return "AU_LBF2";
    case AU_LBF3: return "AU_LBF3";
    case AU_CHLB: return "AU_CHLB";
    case AU_FINAL: return "AU_FINAL";
    default: return "AU_NONE";
  }
}

// A rough share of the whole run for each queue, from the time each took on
// shell32.dll (code 14%, functions 3%, reanalysis 74%, types 8%, the rest 1%).
// The queues are visited in priority order but repeatedly, so a stage is a
// band and the address within the database moves the estimate inside it.
static int stage_percent(atype_t t, ea_t ea, ea_t lo, ea_t hi)
{
  double frac = 0.0;
  if ( ea != BADADDR && hi > lo && ea >= lo && ea < hi )
    frac = double(ea - lo) / double(hi - lo);
  double from, span;
  switch ( t )
  {
    case AU_UNK:
    case AU_CODE:
    case AU_WEAK: from = 0; span = 14; break;
    case AU_PROC:
    case AU_TAIL:
    case AU_FCHUNK: from = 14; span = 3; break;
    case AU_USED:
    case AU_USD2: from = 17; span = 73; break;
    case AU_TYPE: from = 90; span = 7; break;
    case AU_LIBF:
    case AU_LBF2:
    case AU_LBF3:
    case AU_CHLB: from = 97; span = 1; break;
    case AU_FINAL: from = 98; span = 1; break;
    default: from = 0; span = 0; break;
  }
  return int(from + span * frac);
}

static void emit_analysis(atype_t type, ea_t ea, int percent, bool done, bool first, long long elapsed_ms)
{
  Json j;
  j.obj()
    .kv("event", "analysis")
    .kv("stage", done ? "done" : stage_name(type))
    .key("ea").opt_u64(ea, BADADDR)
    .kv("percent", (uint64_t)percent)
    .kv("functions", (uint64_t)get_func_qty())
    .kv("elapsed_ms", (uint64_t)elapsed_ms)
    .kvb("done", done)
    .kvb("start", first)
    .end_obj();
  ev_emit(j.s);
}

static std::atomic<bool> g_stop_auto(false);

void ev_request_stop(void)
{
  g_stop_auto.store(true);
}

bool ev_stop_requested(void)
{
  return g_stop_auto.load();
}

void ev_note_display(ea_t ea)
{
  note_ea(K_DISPLAY, ea);
}

void ev_run_auto(bool always)
{
  const bool trace = qgetenv("IZS_AUTO_TRACE", nullptr);
  const ea_t lo = inf_get_min_ea();
  const ea_t hi = inf_get_max_ea();
  const auto t0 = Clock::now();
  auto last = t0;
  auto ms = [&](Clock::time_point from) {
    return (long long)std::chrono::duration_cast<std::chrono::milliseconds>(Clock::now() - from).count();
  };
  int percent = 0;
  bool announced = false;
  if ( always )
  {
    emit_analysis(AU_NONE, BADADDR, 0, false, true, 0);
    announced = true;
  }
  enable_auto(true);
  uint64_t steps = 0;
  // auto_make_step reports whether it did anything; asking auto_is_ok()
  // before every one of the ~10 million steps of a big DLL costs seconds.
  // The stop flag is sticky: a stop that arrived while the loader was still
  // running must not be lost, and the host is about to close anyway.
  while ( !g_stop_auto.load() && auto_make_step(0, BADADDR) )
  {
    if ( (++steps & 63) != 0 )
      continue;
    if ( ms(last) < 100 )
      continue;
    last = Clock::now();
    auto_display_t d;
    get_auto_display(&d);
    int p = stage_percent(d.type, d.ea, lo, hi);
    if ( p > percent )
      percent = p;
    if ( p > 99 )
      percent = 99;
    if ( trace )
      qeprintf("auto: %lld ms stage=%s ea=%llx raw=%d shown=%d\n", ms(t0), stage_name(d.type),
               (unsigned long long)d.ea, p, percent);
    // A run that proved short never shows; one that lasts is announced here.
    emit_analysis(d.type, d.ea, percent, false, !announced, ms(t0));
    announced = true;
  }
  const bool stopped = g_stop_auto.load();
  if ( stopped )
    enable_auto(false);   // nothing may keep analysing while the database closes
  else
    auto_wait();
  if ( trace )
    qeprintf("auto: finished %lld ms after %llu steps\n", ms(t0), (unsigned long long)steps);
  if ( announced )
    emit_analysis(stopped ? AU_UNK : AU_NONE, BADADDR, stopped ? percent : 100, true, false, ms(t0));
}
