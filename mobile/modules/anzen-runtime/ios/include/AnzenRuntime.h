#ifndef ANZEN_RUNTIME_H
#define ANZEN_RUNTIME_H
#include <stdint.h>

// All strings are UTF-8 and NUL terminated. key points to 32 bytes. The returned JSON
// envelope is owned by Rust and must be released once with anzen_mobile_free.
char *anzen_mobile_execute(const char *root, const char *role, const uint8_t *key,
                           const char *settings, const char *command);
// Live updates: request is {"method":"start"|"targets"|"changes"|"stop", ...}. It never waits
// for a running command. The returned JSON envelope is released with anzen_mobile_free.
char *anzen_mobile_watch(const char *settings, const char *request);
// GPU benchmarks: request is {"method":"backends"} or {"method":"run","backend":"wgpu"|"metal",
// "kernel":"key"|"nonce","seconds":n}. A run blocks for its whole length; call it off the main
// thread. The returned JSON envelope is released with anzen_mobile_free.
char *anzen_mobile_benchmark(const char *request);
void anzen_mobile_free(char *value);
#endif
