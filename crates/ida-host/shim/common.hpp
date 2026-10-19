// State and helpers shared by the shim's translation units: the result
// buffer, the open flag and the guard macros every izs_* command uses.
#pragma once

#include <pro.h>
#include <ida.hpp>
#include <undo.hpp>

#include <string>

#include "json.hpp"

inline std::string g_result;
inline bool g_open = false;
inline bool g_hexrays = false;

inline int ok(Json &j) { g_result = std::move(j.s); return 0; }
inline int ok_null() { g_result = "null"; return 0; }
inline int fail(const char *fmt, ...)
{
  va_list va;
  va_start(va, fmt);
  qstring q;
  q.cat_vsprnt(fmt, va);
  va_end(va);
  g_result.assign(q.c_str(), q.length());
  return 1;
}

#define REQUIRE_DB() do { if ( !g_open ) return fail("no database is open"); } while ( 0 )
#define GUARD_BEGIN try {
#define GUARD_END } catch ( const std::exception &e ) { return fail("exception: %s", e.what()); } \
                    catch ( ... ) { return fail("unknown exception in IDA"); }

inline std::string qs(const qstring &q) { return std::string(q.c_str(), q.length()); }

inline void undo_point()
{
  create_undo_point(nullptr, 0);
}

// Defined in shim.cpp.
std::string short_name(ea_t ea);
extern "C" void emit_xref(Json &j, ea_t from, ea_t to, uchar type, ea_t text_at);
