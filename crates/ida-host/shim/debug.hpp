// Debugger session helpers shared with the open/save/close paths in shim.cpp.
#pragma once

#include <pro.h>
#include <ida.hpp>

// True while a debuggee exists (running or suspended).
bool dbg_process_alive();
// True when the image base differs from the base recorded before the
// session started (the database is rebased to the debuggee).
bool dbg_db_rebased();
// Ends any session: terminates the debuggee, waits for the exit event and
// rebases the database back. Returns false if the database is still rebased.
bool dbg_shutdown();
