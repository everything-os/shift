#pragma once

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct TabClientHandle TabClientHandle;

typedef struct TabFrameTarget {
	uint32_t framebuffer;
	uint32_t texture;
	int32_t width;
	int32_t height;
} TabFrameTarget;

typedef enum TabAcquireResult {
	TAB_ACQUIRE_OK = 0,
	TAB_ACQUIRE_NO_BUFFERS = 1,
	TAB_ACQUIRE_ERROR = 2,
} TabAcquireResult;

// Connect to the Tab socket at `socket_path` (UTF-8) and authenticate with `token`.
// Pass NULL for `socket_path` to use the default /tmp/shift.sock. `token` must be non-null.
TabClientHandle* tab_client_connect(const char* socket_path, const char* token);

// Convenience helper for default socket path.
static inline TabClientHandle* tab_client_connect_default(const char* token) {
	return tab_client_connect(NULL, token);
}

// Disconnect and free resources associated with the handle.
void tab_client_disconnect(TabClientHandle* handle);

// Retrieve and clear the last error message. Caller must free via tab_client_string_free.
char* tab_client_take_error(TabClientHandle* handle);

char* tab_client_get_server_name(TabClientHandle* handle);
char* tab_client_get_protocol_name(TabClientHandle* handle);
char* tab_client_get_session_json(TabClientHandle* handle);
size_t tab_client_get_monitor_count(TabClientHandle* handle);
char* tab_client_get_monitor_id(TabClientHandle* handle, size_t index);
bool tab_client_send_ready(TabClientHandle* handle);
TabAcquireResult tab_client_acquire_frame(TabClientHandle* handle, const char* monitor_id, TabFrameTarget* target);
bool tab_client_swap_buffers(TabClientHandle* handle, const char* monitor_id);
bool tab_client_set_cursor_fb(
	TabClientHandle* handle,
	const char* monitor_id,
	uint32_t width,
	uint32_t height,
	int32_t hotspot_x,
	int32_t hotspot_y,
	const uint8_t* pixels,
	size_t pixel_len);
bool tab_client_set_cursor_pos(TabClientHandle* handle, const char* monitor_id, int32_t x, int32_t y);

int  tab_client_get_socket_fd(TabClientHandle* handle);
int  tab_client_get_swap_fd(TabClientHandle* handle);
bool tab_client_process_socket_events(TabClientHandle* handle);
bool tab_client_process_swap_events(TabClientHandle* handle);

// Free an error string returned by tab_client_take_error.
void tab_client_string_free(char* s);

#ifdef __cplusplus
} // extern "C"
#endif
