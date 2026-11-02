/*
 * kartta.h: the C ABI of the kartta code graph engine, as the kartta-ffi static library exports it.
 *
 * A handle names a tree. Every method goes through kartta_call: a method name and a JSON object of params in, a JSON
 * answer out, either {"ok": <answer>} or {"err": {"code": <code>, "message": <message>}}. WIRE.md, beside this header
 * in the release archive and in crates/kartta-ffi of the kartta repository, states every method, field, default, error
 * code and enumerated value; the version method lists the methods a library answers.
 *
 * Compatibility: a method refuses a param it does not know with the code "invalid_params", but an answer may gain a key
 * in any release, so a consumer ignores the keys it does not know.
 *
 * Threads: calls on one handle may run at once from any threads. kartta_call blocks its thread until the method
 * answers, which for the build method is the whole walk of the tree, so a consumer that must stay responsive calls it
 * on a thread of its own. Builds run one at a time, and a query during a build answers the code "building". A refresh
 * holds the engine alone, so a query during a refresh waits for it to finish. A build that asks to watch starts a
 * thread of the library's own, which refreshes the engine from each change the operating system reports until the next
 * build or kartta_free, and ends within about a second of either. kartta_free must not run while a call on the same
 * handle runs, and nothing may use the handle after it.
 *
 * Stopping: before kartta_free, call the cancel method with {}, which cancels every build, grep and refresh in flight,
 * then join every thread that is inside a call on the handle. A grep past its last file and a refresh past its walk
 * finish all the same, and cancel stops no other query, so the join waits for those. A call that enters after the
 * cancel runs to its end, so a thread that may still be starting one names it with an id and cancels that id, which
 * stops it when it enters, unless 1,024 later cancels that stopped nothing, of its method through the same key, id or
 * binding_id, came first.
 *
 * Memory: the library copies what it needs of every argument before returning, so the caller's buffers are borrowed
 * for the call only. Every KarttaString the library returns is the caller's, to be freed exactly once with
 * kartta_string_free. A string outlives the handle that answered it: kartta_free frees none, and a string may be freed
 * after it.
 *
 * Panics: no panic crosses these functions. A panic inside kartta_open or kartta_call answers the code "internal", and
 * the handle stays usable; the frees, which answer nothing, drop one. This holds only for a library built with
 * unwinding panics, the Rust default.
 */

#ifndef KARTTA_H
#define KARTTA_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* The version of these signatures and ownership rules this header declares, which kartta_abi_version must equal. */
#define KARTTA_ABI_VERSION 1

/* A handle on one tree, opened by kartta_open and freed by kartta_free. */
typedef struct KarttaHandle KarttaHandle;

/* UTF-8 JSON the library allocated: len bytes at ptr, with no terminating NUL. A null ptr holds nothing. */
typedef struct KarttaString {
    uint8_t *ptr;
    size_t len;
} KarttaString;

/* The ABI version of the library linked, to compare with KARTTA_ABI_VERSION before any other call. */
uint32_t kartta_abi_version(void);

/*
 * Opens a handle on the tree params names: params_len bytes of the JSON object {"root": "<path>"}. The root is
 * required, so a null params with a zero length, which reads as {} as it does for kartta_call, answers the code
 * "invalid_params". Nothing is walked until the build method runs.
 *
 * Returns the handle, and writes a null KarttaString to *error when error is not null. On failure returns null and
 * writes the JSON object {"code": <code>, "message": <message>} to *error, for the caller to free; a null error
 * discards it. The codes are "invalid_request", "invalid_params", "invalid_root" and "internal".
 */
KarttaHandle *kartta_open(const uint8_t *params, size_t params_len, KarttaString *error);

/*
 * Answers method, a NUL-terminated UTF-8 name, with params: params_len bytes of a JSON object. A null params with a
 * zero length, empty text, text of whitespace alone and the JSON null each read as {}. Never returns a null
 * KarttaString: a null handle or method answers the code "invalid_request". Blocks until the method answers.
 */
KarttaString kartta_call(const KarttaHandle *handle, const char *method, const uint8_t *params, size_t params_len);

/* Frees a KarttaString the library returned. A null string is ignored. */
void kartta_string_free(KarttaString string);

/* Frees a handle kartta_open returned, and the engine it holds. A null handle is ignored. */
void kartta_free(KarttaHandle *handle);

#ifdef __cplusplus
}
#endif

#endif
