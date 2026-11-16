#ifndef HANDSMCP_MACOS_BRIDGE_H
#define HANDSMCP_MACOS_BRIDGE_H

#ifdef __cplusplus
extern "C" {
#endif

void handsmcp_free_cstr(char *ptr);
char *handsmcp_macos_doctor(void);
char *handsmcp_macos_list_apps(void);
char *handsmcp_macos_activate_app(const char *bundle_id);
char *handsmcp_macos_launch_app(const char *bundle_id);
char *handsmcp_macos_inspect_app(const char *bundle_id, int window_index, int max_elements, int max_depth);
char *handsmcp_macos_capture_and_ocr(const char *bundle_id, int window_index, int ocr_mode, int encode_png);
char *handsmcp_macos_render_som(const char *png_b64_or_cap_id, const char *marks_json, double win_x, double win_y, double win_w, double win_h);
char *handsmcp_macos_perform_action(const char *req_json);
char *handsmcp_macos_check_point(const char *bundle_id, double x, double y);
char *handsmcp_macos_open_url(const char *bundle_id, const char *url);
void handsmcp_macos_tick(void);

#ifdef __cplusplus
}
#endif

#endif
