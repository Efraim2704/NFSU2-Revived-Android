#pragma once

#include <android/input.h>
#include <jni.h>

#ifdef __cplusplus
extern "C" {
#endif

void nfs_gamepad_camera_init(void);
void nfs_gamepad_camera_on_motion(const AInputEvent* event);
void nfs_gamepad_camera_feed(float rx, float ry);

#ifdef __cplusplus
}
#endif
