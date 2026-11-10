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
// A vanity-search benchmark: request is {"engine":"gpu"|"cpu","seconds":n}. It blocks for its
// whole length; call it off the main thread. The returned JSON envelope is released with
// anzen_mobile_free.
char *anzen_mobile_benchmark(const char *request);
// The Bluetooth link to a Ledger signer: request is {"method":"connect"|"request"|"disconnect",
// ...}. It blocks while the person reviews on the Ledger; call it off the main thread. The
// returned JSON envelope is released with anzen_mobile_free.
char *anzen_mobile_ledger(const char *request);
// The link to a Trezor signer: request is {"method":"connect"|"pair"|"request"|"disconnect",
// ...}. root is the directory the phone's devices are stored in. It blocks while the person
// reviews on the Trezor; call it off the main thread. The returned JSON envelope is released
// with anzen_mobile_free.
char *anzen_mobile_trezor(const char *root, const char *request);
void anzen_mobile_free(char *value);
#endif
