/* NOLINTBEGIN */
#ifndef TAB_CLIENT_H
#define TAB_CLIENT_H

#include <cstdio>
#ifdef __cplusplus
extern "C" {
#endif

#include <stdint.h>
#include <stdbool.h>
#include <stddef.h>

/* ============================================================================
 * OPAQUE HANDLE
 * ============================================================================
 */

typedef struct TabClientHandle TabClientHandle;

/* ============================================================================
 * INPUT ENUMS
 * ============================================================================
 */

typedef enum {
    TAB_INPUT_KIND_POINTER_MOTION = 0,
    TAB_INPUT_KIND_POINTER_MOTION_ABSOLUTE = 1,
    TAB_INPUT_KIND_POINTER_BUTTON = 2,
    TAB_INPUT_KIND_POINTER_AXIS = 3,
    TAB_INPUT_KIND_KEY = 6,
    TAB_INPUT_KIND_TOUCH_DOWN = 7,
    TAB_INPUT_KIND_TOUCH_UP = 8,
    TAB_INPUT_KIND_TOUCH_MOTION = 9,
    TAB_INPUT_KIND_TOUCH_FRAME = 10,
    TAB_INPUT_KIND_TOUCH_CANCEL = 11,
    TAB_INPUT_KIND_TABLET_TOOL_PROXIMITY = 12,
    TAB_INPUT_KIND_TABLET_TOOL_AXIS = 13,
    TAB_INPUT_KIND_TABLET_TOOL_TIP = 14,
    TAB_INPUT_KIND_TABLET_TOOL_BUTTON = 15,
    TAB_INPUT_KIND_TABLET_PAD_BUTTON = 16,
    TAB_INPUT_KIND_TABLET_PAD_RING = 17,
    TAB_INPUT_KIND_TABLET_PAD_STRIP = 18,
    TAB_INPUT_KIND_SWITCH_TOGGLE = 19,
    TAB_INPUT_KIND_GESTURE_SWIPE_BEGIN,
    TAB_INPUT_KIND_GESTURE_SWIPE_UPDATE,
    TAB_INPUT_KIND_GESTURE_SWIPE_END,

    TAB_INPUT_KIND_GESTURE_PINCH_BEGIN,
    TAB_INPUT_KIND_GESTURE_PINCH_UPDATE,
    TAB_INPUT_KIND_GESTURE_PINCH_END,

    TAB_INPUT_KIND_GESTURE_HOLD_BEGIN,
    TAB_INPUT_KIND_GESTURE_HOLD_END,
} TabInputEventKind;

typedef enum {
    TAB_BUTTON_PRESSED = 0,
    TAB_BUTTON_RELEASED = 1,
} TabButtonState;

typedef enum {
    TAB_AXIS_VERTICAL = 0,
    TAB_AXIS_HORIZONTAL = 1,
} TabAxisOrientation;

typedef enum {
    TAB_AXIS_SOURCE_WHEEL = 0,
    TAB_AXIS_SOURCE_FINGER = 1,
    TAB_AXIS_SOURCE_CONTINUOUS = 2,
    TAB_AXIS_SOURCE_WHEEL_TILT = 3,
} TabAxisSource;

typedef enum {
    TAB_KEY_PRESSED = 0,
    TAB_KEY_RELEASED = 1,
} TabKeyState;

typedef enum {
    TAB_TIP_DOWN = 0,
    TAB_TIP_UP = 1,
} TabTipState;

typedef enum {
    TAB_SWITCH_LID = 0,
    TAB_SWITCH_TABLET_MODE = 1,
} TabSwitchType;

typedef enum {
    TAB_SWITCH_ON = 0,
    TAB_SWITCH_OFF = 1,
} TabSwitchState;

/* ============================================================================
 * INPUT STRUCTS
 * ============================================================================
 */

typedef struct {
    uint32_t device;
    uint64_t time_usec;
    double x, y;
    double dx, dy;
    double unaccel_dx, unaccel_dy;
} TabInputPointerMotion;

typedef struct {
    uint32_t device;
    uint64_t time_usec;
    double x, y;
    double x_transformed, y_transformed;
} TabInputPointerMotionAbsolute;

typedef struct {
    uint32_t device;
    uint64_t time_usec;
    uint32_t button;
    TabButtonState state;
} TabInputPointerButton;

typedef struct {
    uint32_t device;
    uint64_t time_usec;
    TabAxisOrientation orientation;
    double delta;
    int32_t delta_discrete;
    TabAxisSource source;
} TabInputPointerAxis;



typedef struct {
    uint32_t device;
    uint64_t time_usec;
    uint32_t key;
    TabKeyState state;
} TabInputKey;

typedef struct {
    int32_t id;
    double x, y;
    double x_transformed, y_transformed;
} TabTouchContact;

typedef struct {
    uint32_t device;
    uint64_t time_usec;
    TabTouchContact contact;
} TabInputTouchDown;

typedef struct {
    uint32_t device;
    uint64_t time_usec;
    TabTouchContact contact;
} TabInputTouchMotion;

typedef struct {
    uint32_t device;
    uint64_t time_usec;
    int32_t contact_id;
} TabInputTouchUp;

typedef struct {
    uint64_t time_usec;
} TabInputTouchFrame;

typedef struct {
    uint64_t time_usec;
} TabInputTouchCancel;

typedef struct {
    uint64_t serial;
    uint8_t tool_type;
} TabTabletTool;

typedef struct {
    uint32_t device;
    uint64_t time_usec;
    bool in_proximity;
    TabTabletTool tool;
} TabInputTabletToolProximity;

typedef struct {
    double x, y;
    double pressure;
    double distance;
    double tilt_x, tilt_y;
    double rotation;
    double slider;
    double wheel_delta;
} TabTabletToolAxes;

typedef struct {
    uint32_t device;
    uint64_t time_usec;
    TabTabletTool tool;
    TabTabletToolAxes axes;
} TabInputTabletToolAxis;

typedef struct {
    uint32_t device;
    uint64_t time_usec;
    TabTabletTool tool;
    TabTipState state;
} TabInputTabletToolTip;

typedef struct {
    uint32_t device;
    uint64_t time_usec;
    TabTabletTool tool;
    uint32_t button;
    TabButtonState state;
} TabInputTabletToolButton;

typedef struct {
    uint32_t device;
    uint64_t time_usec;
    uint32_t button;
    TabButtonState state;
} TabInputTabletPadButton;

typedef struct {
    uint32_t device;
    uint64_t time_usec;
    uint32_t ring;
    double position;
    TabAxisSource source;
} TabInputTabletPadRing;

typedef struct {
    uint32_t device;
    uint64_t time_usec;
    uint32_t strip;
    double position;
    TabAxisSource source;
} TabInputTabletPadStrip;


typedef struct {
    uint32_t device;
    uint64_t time_usec;
    TabSwitchType switch_type;
    TabSwitchState state;
} TabInputSwitchToggle;

// Gestures

typedef struct {
    uint32_t device;
    uint64_t time_usec;
    uint32_t fingers;
} TabInputGestureSwipeBegin;

typedef struct {
    uint32_t device;
    uint64_t time_usec;
    uint32_t fingers;
    double dx, dy;
} TabInputGestureSwipeUpdate;

typedef struct {
    uint32_t device;
    uint64_t time_usec;
    bool cancelled;
} TabInputGestureSwipeEnd;
typedef struct {
    uint32_t device;
    uint64_t time_usec;
    uint32_t fingers;
} TabInputGesturePinchBegin;

typedef struct {
    uint32_t device;
    uint64_t time_usec;
    uint32_t fingers;
    double dx, dy;
    double scale;
    double rotation;
} TabInputGesturePinchUpdate;

typedef struct {
    uint32_t device;
    uint64_t time_usec;
    bool cancelled;
} TabInputGesturePinchEnd;
typedef struct {
    uint32_t device;
    uint64_t time_usec;
    uint32_t fingers;
} TabInputGestureHoldBegin;

typedef struct {
    uint32_t device;
    uint64_t time_usec;
    bool cancelled;
} TabInputGestureHoldEnd;

/* ============================================================================
 * INPUT EVENT UNION
 * ============================================================================
 */

typedef union {
    TabInputPointerMotion pointer_motion;
    TabInputPointerMotionAbsolute pointer_motion_absolute;
    TabInputPointerButton pointer_button;
    TabInputPointerAxis pointer_axis;
    TabInputKey key;
    TabInputTouchDown touch_down;
    TabInputTouchUp touch_up;
    TabInputTouchMotion touch_motion;
    TabInputTouchFrame touch_frame;
    TabInputTouchCancel touch_cancel;
    TabInputTabletToolProximity tablet_tool_proximity;
    TabInputTabletToolAxis tablet_tool_axis;
    TabInputTabletToolTip tablet_tool_tip;
    TabInputTabletToolButton tablet_tool_button;
    TabInputTabletPadButton tablet_pad_button;
    TabInputTabletPadRing tablet_pad_ring;
    TabInputTabletPadStrip tablet_pad_strip;
    
    TabInputSwitchToggle switch_toggle;
    TabInputGestureSwipeBegin swipe_begin;
    TabInputGestureSwipeUpdate swipe_update;
    TabInputGestureSwipeEnd swipe_end;

    TabInputGesturePinchBegin pinch_begin;
    TabInputGesturePinchUpdate pinch_update;
    TabInputGesturePinchEnd pinch_end;

    TabInputGestureHoldBegin hold_begin;
    TabInputGestureHoldEnd hold_end;
} TabInputEventData;

typedef struct {
    TabInputEventKind kind;
    TabInputEventData data;
} TabInputEvent;

/* ============================================================================
 * MONITORS
 * ============================================================================
 */

typedef struct {
    const char *id;
    int32_t width;
    int32_t height;
    int32_t refresh_rate;
    const char *name;
} TabMonitorInfo;

/* ============================================================================
 * SESSIONS
 * ============================================================================
 */

typedef enum {
    TAB_SESSION_ROLE_ADMIN = 0,
    TAB_SESSION_ROLE_SESSION = 1,
} TabSessionRole;

typedef enum {
    TAB_SESSION_LIFECYCLE_PENDING = 0,
    TAB_SESSION_LIFECYCLE_LOADING = 1,
    TAB_SESSION_LIFECYCLE_OCCUPIED = 2,
    TAB_SESSION_LIFECYCLE_CONSUMED = 3,
} TabSessionLifecycle;

typedef struct {
    const char *id;
    TabSessionRole role;
    const char *display_name;
    TabSessionLifecycle state;
} TabSessionInfo;

/* ============================================================================
 * EVENTS
 * ============================================================================
 */

typedef enum {
    TAB_ACQUIRE_OK = 0,
    TAB_ACQUIRE_NO_BUFFERS = 1,
    TAB_ACQUIRE_ERROR = 2,
} TabAcquireResult;

typedef enum {
    TAB_EVENT_BUFFER_RELEASED = 0,
    TAB_EVENT_MONITOR_ADDED = 1,
    TAB_EVENT_MONITOR_REMOVED = 2,
    TAB_EVENT_SESSION_STATE = 3,
    TAB_EVENT_INPUT = 4,
    TAB_EVENT_SESSION_CREATED = 5,
    TAB_EVENT_SESSION_AWAKE = 6,
    TAB_EVENT_SESSION_SLEEP = 7,
    TAB_EVENT_SESSION_ACTIVE = 8,
} TabEventType;

typedef struct {
    const char *monitor_id;
    uint32_t buffer_index;
    int32_t release_fence_fd;
} TabBufferRelease;

typedef union {
    TabBufferRelease buffer_released;
    TabMonitorInfo monitor_added;
    const char *monitor_removed;
    TabSessionInfo session_state;
    const char *session_awake;
    const char *session_sleep;
    const char *session_active;
    TabInputEvent input;
    const char *session_created_token;
} TabEventData;

typedef struct {
    TabEventType event_type;
    TabEventData data;
} TabEvent;

/* ============================================================================
 * FRAME TARGETS
 * ============================================================================
 */

typedef struct {
    int fd;
    int stride;
    int offset;
    uint32_t fourcc;
} TabDmabuf;

typedef struct {
    uint32_t framebuffer;
    uint32_t texture;
    int32_t width;
    int32_t height;
    uint32_t buffer_index;
    TabDmabuf dmabuf;
} TabFrameTarget;
/* ============================================================================
 * API
 * ============================================================================
 */

TabClientHandle *tab_client_connect(const char *socket_path, const char *token);
TabClientHandle *tab_client_connect_default(const char *token);
void tab_client_disconnect(TabClientHandle *handle);

void tab_client_string_free(const char *s);
char *tab_client_take_error(TabClientHandle *handle);

char *tab_client_get_server_name(TabClientHandle *handle);
char *tab_client_get_protocol_name(TabClientHandle *handle);

size_t tab_client_get_monitor_count(TabClientHandle *handle);
char *tab_client_get_monitor_id(TabClientHandle *handle, size_t index);
TabMonitorInfo tab_client_get_monitor_info(TabClientHandle *handle, const char *monitor_id);
void tab_client_free_monitor_info(TabMonitorInfo *info);
TabSessionInfo tab_client_get_session(TabClientHandle *handle);
void tab_client_free_session_info(TabSessionInfo *session_info);
bool tab_client_send_ready(TabClientHandle *handle);

size_t tab_client_poll_events(TabClientHandle *handle);
bool tab_client_next_event(TabClientHandle *handle, TabEvent *event);
void tab_client_free_event_strings(TabEvent *event);

TabAcquireResult tab_client_acquire_frame(
    TabClientHandle *handle,
    const char *monitor_id,
    TabFrameTarget *target
);

bool tab_client_request_buffer(
    TabClientHandle *handle,
    const char *monitor_id,
    int acquire_fence_fd
);

int tab_client_get_swap_fd(TabClientHandle *handle);
int tab_client_get_socket_fd(TabClientHandle *handle);
int tab_client_drm_fd(TabClientHandle *handle);
#ifdef __cplusplus
}
#endif

#endif /* TAB_CLIENT_H */

/* NOLINTEND */
