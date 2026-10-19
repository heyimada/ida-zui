// Unsolicited host-to-UI events: IDA's message output, coalesced database
// changes and auto-analysis progress. Every event is one JSON line with an
// "event" key and no "id", written to the protocol stream through the emitter
// that ida-host registers. See events.cpp.
#pragma once

#include <pro.h>
#include <ida.hpp>

#include <string>
#include <vector>

enum EvOrigin
{
  EV_REQUEST = 1, // an ordinary protocol request
  EV_PYTHON = 2,  // Python executing (console, script file)
};

typedef void (*ev_emit_fn)(const char *line, size_t len);

void ev_set_emitter(ev_emit_fn fn);
void ev_emit(const std::string &json);

// After init_library: listens for IDA's messages and starts the flusher.
void ev_init();
// Database open: starts reporting database changes.
void ev_hook_idb();
void ev_unhook_idb();

// Request bracketing: the origin tags database changes made meanwhile.
void ev_begin_request(int origin);
// Emits pending log lines and database changes.
void ev_end_request();

// The lines IDA prints between begin and end, for requests that quote them.
void ev_capture_begin();
std::vector<std::string> ev_capture_end();

// Python bookkeeping for the reply of one evaluation.
void ev_python_begin();
bool ev_python_changed();

// The address `idc.here()` reports.
void ev_set_screen_ea(ea_t ea);

// Runs the auto-analysis queues to completion in small steps, reporting
// progress. With `always` the start and the end are reported even for a short
// run (the initial analysis); otherwise only runs longer than ~100 ms are.
void ev_run_auto(bool always);
