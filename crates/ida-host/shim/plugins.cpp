// Plugins: the loader's plugin list and running one by name or path. idalib
// has no Qt UI, so a plugin that creates widgets or asks questions through
// the UI does nothing; the reply carries what IDA logged so the caller can
// tell.

#include <pro.h>
#include <ida.hpp>
#include <kernwin.hpp>
#include <loader.hpp>

#include <string>

#include "common.hpp"
#include "events.hpp"

static std::string cstr(const char *s)
{
  return s != nullptr ? std::string(s) : std::string();
}

// The hotkey as IDA writes it ("Ctrl-Alt-F"), when the plugin says one.
static std::string hotkey_of(const plugin_info_t *pi)
{
  if ( pi->entry != nullptr && pi->entry->wanted_hotkey != nullptr )
    return pi->entry->wanted_hotkey;
  return std::string();
}

extern "C" int izs_plugin_list(void)
{
  GUARD_BEGIN
  Json j;
  j.arr();
  for ( const plugin_info_t *pi = get_plugins(); pi != nullptr; pi = pi->next )
  {
    j.obj()
      .kv("name", cstr(pi->name))
      .kv("org_name", cstr(pi->org_name))
      .kv("id", qs(pi->idaplg_name))
      .kv("path", cstr(pi->path))
      .kv("hotkey", hotkey_of(pi))
      .kv("hotkey_code", (uint64_t)pi->hotkey)
      .kv("arg", (uint64_t)pi->arg)
      .kvi("flags", pi->flags)
      .kvb("loaded", pi->entry != nullptr)
      .kv("comment", cstr(pi->comment));
    if ( pi->entry != nullptr )
      j.kv("help", cstr(pi->entry->help));
    j.end_obj();
  }
  j.end_arr();
  return ok(j);
  GUARD_END
}

static bool mentions_gui(const std::string &text)
{
  static const char *const needles[] = {
    "pyqt", "pyside", "qtwidgets", "qtgui", "qtcore", "qapplication", "qwidget", "qt platform",
    "no qt", "graphical", "ida_kernwin.ask_", "widget",
  };
  std::string low;
  for ( char c : text )
    low += (char)tolower((unsigned char)c);
  for ( const char *n : needles )
    if ( low.find(n) != std::string::npos )
      return true;
  return false;
}

// Runs the plugin whose list `path` (or short name) matches. The plugin is
// loaded first when it is not in memory yet.
extern "C" int izs_plugin_run(const char *which, uint64_t arg)
{
  REQUIRE_DB();
  GUARD_BEGIN
  plugin_info_t *found = nullptr;
  for ( plugin_info_t *pi = get_plugins(); pi != nullptr; pi = pi->next )
  {
    if ( cstr(pi->path) == which || cstr(pi->name) == which )
    {
      found = pi;
      break;
    }
  }
  if ( found == nullptr )
    return fail("no such plugin: %s", which);
  bool was_loaded = found->entry != nullptr;
  ev_capture_begin();
  ev_python_begin();
  undo_point();
  plugin_t *p = find_plugin(found->path, true);
  bool ran = false;
  if ( p != nullptr )
    ran = run_plugin(p, (size_t)arg);
  ev_run_auto(false);
  std::vector<std::string> log = ev_capture_end();
  bool changed = ev_python_changed();
  std::string all;
  for ( const std::string &l : log )
    all += l + "\n";
  bool failed = all.find("Failed while executing") != std::string::npos || all.find("Traceback (most recent") != std::string::npos;
  Json j;
  j.obj()
    .kv("name", cstr(found->name))
    .kv("path", cstr(found->path))
    .kvb("was_loaded", was_loaded)
    .kvb("loadable", p != nullptr)
    .kvb("returned", ran)
    .kvb("failed", failed)
    .kvb("changed", changed)
    .kvb("needs_gui", mentions_gui(all))
    .key("log").arr();
  for ( const std::string &l : log )
    j.str(l);
  j.end_arr().end_obj();
  return ok(j);
  GUARD_END
}
