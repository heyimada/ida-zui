// Reserves the process's original stdin/stdout for the protocol and points
// the standard streams elsewhere, so anything IDA prints (it shares the UCRT
// with us) lands on stderr instead of corrupting the protocol stream.
// Kept apart from shim.cpp because <windows.h> clashes with the IDA headers.

#include <windows.h>
#include <fcntl.h>
#include <io.h>
#include <stdio.h>

extern "C" unsigned long izs_thread_id(void)
{
  return GetCurrentThreadId();
}

// Raises KeyboardInterrupt in thread `tid` of the embedded Python. Takes the
// GIL from the calling (non-IDA) thread, so the running script must be
// executing bytecode or waiting without the GIL; a long C call that holds it
// delays the interrupt. Returns 0 on success.
extern "C" int izs_py_interrupt(unsigned long tid)
{
  HMODULE py = nullptr;
  for ( int minor = 8; minor <= 16 && py == nullptr; minor++ )
  {
    char name[32];
    sprintf_s(name, "python3%d.dll", minor);
    py = GetModuleHandleA(name);
  }
  if ( py == nullptr )
    return 2;
  typedef int (*ensure_t)(void);
  typedef void (*release_t)(int);
  typedef int (*set_async_t)(unsigned long, void *);
  ensure_t ensure = (ensure_t)GetProcAddress(py, "PyGILState_Ensure");
  release_t release = (release_t)GetProcAddress(py, "PyGILState_Release");
  set_async_t set_async = (set_async_t)GetProcAddress(py, "PyThreadState_SetAsyncExc");
  void **exc = (void **)GetProcAddress(py, "PyExc_KeyboardInterrupt");
  if ( ensure == nullptr || release == nullptr || set_async == nullptr || exc == nullptr )
    return 3;
  int state = ensure();
  int n = set_async(tid, *exc);
  release(state);
  return n == 1 ? 0 : 4;
}

extern "C" int izs_take_stdio(HANDLE *proto_in, HANDLE *proto_out)
{
  HANDLE self = GetCurrentProcess();
  if ( !DuplicateHandle(self, GetStdHandle(STD_INPUT_HANDLE), self, proto_in, 0, FALSE, DUPLICATE_SAME_ACCESS) )
    return 1;
  if ( !DuplicateHandle(self, GetStdHandle(STD_OUTPUT_HANDLE), self, proto_out, 0, FALSE, DUPLICATE_SAME_ACCESS) )
    return 2;

  fflush(stdout);
  if ( _dup2(_fileno(stderr), _fileno(stdout)) != 0 )
    return 3;
  SetStdHandle(STD_OUTPUT_HANDLE, GetStdHandle(STD_ERROR_HANDLE));
  setvbuf(stdout, nullptr, _IONBF, 0);

  int nul = _open("NUL", _O_RDONLY);
  if ( nul >= 0 )
  {
    _dup2(nul, _fileno(stdin));
    _close(nul);
  }
  HANDLE hnul = CreateFileA("NUL", GENERIC_READ, FILE_SHARE_READ | FILE_SHARE_WRITE, nullptr, OPEN_EXISTING, 0, nullptr);
  if ( hnul != INVALID_HANDLE_VALUE )
    SetStdHandle(STD_INPUT_HANDLE, hnul);
  return 0;
}
