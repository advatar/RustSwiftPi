#ifndef PI_SWIFT_FFI_H
#define PI_SWIFT_FFI_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

// Returns 0 on success and writes a newly-allocated UTF-8 string to `out_response`.
// Returns non-zero on failure and writes a newly-allocated UTF-8 string to `out_error`.
//
// All `const char*` inputs may be null unless otherwise documented.
// `prompt` must be non-null and non-empty.
int32_t pi_run_prompt(
    const char *api_key,
    const char *base_url,
    const char *model,
    const char *system_prompt,
    const char *cwd,
    const char *prompt,
    char **out_response,
    char **out_error
);

// Returns 0 on success and writes a newly-allocated UTF-8 JSON string to `out_transcript_json`.
// Returns non-zero on failure and writes a newly-allocated UTF-8 string to `out_error`.
//
// All `const char*` inputs may be null unless otherwise documented.
// `prompt` must be non-null and non-empty.
int32_t pi_run_prompt_transcript_json(
    const char *api_key,
    const char *base_url,
    const char *model,
    const char *system_prompt,
    const char *cwd,
    const char *prompt,
    char **out_transcript_json,
    char **out_error
);

// Frees a string allocated by this library (returned via `pi_run_prompt*`).
void pi_string_free(char *s);

#ifdef __cplusplus
} // extern "C"
#endif

#endif // PI_SWIFT_FFI_H

