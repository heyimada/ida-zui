/* Harmless debug target for the debugger E2E: loops over a few small
 * functions, prints, raises one caught exception and exits with code 7.
 * Built by scripts/build-debuggee.ps1. Usage: izs_debuggee [iterations] [mode]
 * mode: "x" raises a caught exception, "crash" writes through a null
 * pointer (the test terminates it), "spin" never exits (the test terminates
 * it). It touches no files and no network. */
#include <windows.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

__declspec(dllexport) __declspec(noinline) int izs_leaf(int x)
{
    return x * 2 + 1;
}

__declspec(dllexport) __declspec(noinline) int izs_outer(int x)
{
    return izs_leaf(x) + 1;
}

static DWORD WINAPI worker(LPVOID arg)
{
    (void)arg;
    Sleep(30);
    return 0;
}

int main(int argc, char **argv)
{
    int iters = argc > 1 ? atoi(argv[1]) : 5;
    int total = 0;
    const char *mode = argc > 2 ? argv[2] : "";
    HANDLE t = CreateThread(NULL, 0, worker, NULL, 0, NULL);
    OutputDebugStringA("izs-debuggee: started\n");
    for (int i = 0; i < iters; i++) {
        total += izs_outer(i);
        Sleep(10);
    }
    if (strcmp(mode, "x") == 0) {
        __try {
            RaiseException(0xE0000001, 0, 0, NULL);
        } __except (EXCEPTION_EXECUTE_HANDLER) {
            total += 1000;
        }
    }
    if (strcmp(mode, "crash") == 0) {
        *(volatile int *)0 = total;
    }
    while (strcmp(mode, "spin") == 0) {
        Sleep(10);
    }
    if (t) {
        WaitForSingleObject(t, 1000);
        CloseHandle(t);
    }
    printf("izs-debuggee: total=%d\n", total);
    return 7;
}
