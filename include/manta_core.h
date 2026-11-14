#ifndef MANTA_CORE_H
#define MANTA_CORE_H

#include <stdint.h>
#include <stdbool.h>

#ifdef __cplusplus
extern "C" {
#endif

/**
 * Manta Core v3.0 — Universal Bolt-On C-ABI Header
 */

typedef int32_t (*MantaNetFetchCallback)(
    const char* method,
    const char* url,
    const char* req_headers_crlf,
    void* user_ctx,
    void (*on_response_meta)(void* user_ctx, uint16_t status, const char* headers_crlf),
    void (*on_body_chunk)(void* user_ctx, const uint8_t* ptr, uint32_t len, bool is_eof)
);

uint32_t manta_abi_init_engine(const char* storage_path, const char* download_dir);
uint32_t manta_abi_get_worker_cores(void);
void     manta_abi_register_net_transport(MantaNetFetchCallback callback);

/* Non-Destructive Hot-Active Tab Multiplexing */
uint32_t manta_abi_create_tab(void);
bool     manta_abi_switch_tab(uint32_t tab_id);
bool     manta_abi_close_tab(uint32_t tab_id);
uint32_t manta_abi_active_tab_id(void);
uint32_t manta_abi_open_tab_count(void);
char*    manta_abi_get_memory_stats_json(void);

/* High-Velocity Kernel Script & Web Compatibility Compiler */
char*    manta_abi_compile_kernel_script(const char* origin_url);

/* Full End-to-End Page Navigation (HTML + External CSS/JS via Keep-Alive) & Chunk Streaming */
char*    manta_abi_navigate_url(uint32_t tab_id, const char* url);
char*    manta_abi_stream_html_chunk(uint32_t tab_id, const char* url, const char* html_chunk, bool is_eof);
char*    manta_abi_render_viewport_frame(uint32_t tab_id, float viewport_w, float scroll_y, float viewport_h);

/* Navigation, Find-in-Page & Reader Mode */
char*    manta_abi_resolve_omnibox(const char* raw_input);
bool     manta_abi_can_go_back(uint32_t tab_id);
bool     manta_abi_can_go_forward(uint32_t tab_id);
char*    manta_abi_go_back(uint32_t tab_id);
char*    manta_abi_go_forward(uint32_t tab_id);
uint32_t manta_abi_find_in_page(uint32_t tab_id, const char* query);
char*    manta_abi_get_reader_text(uint32_t tab_id);

/* Interactive Forms, Inputs, Events, Auto-Saved Drafts & Binary-Safe File Upload */
bool     manta_abi_focus_node(uint32_t tab_id, uint32_t node_id);
bool     manta_abi_set_input_value(uint32_t tab_id, uint32_t node_id, const char* value);
bool     manta_abi_toggle_checkable(uint32_t tab_id, uint32_t node_id);
bool     manta_abi_dispatch_node_event(uint32_t tab_id, uint32_t node_id, const char* event_type);
bool     manta_abi_attach_file_to_input(uint32_t tab_id, uint32_t node_id, const char* filename, const char* mime_type, const uint8_t* data_ptr, uint32_t data_len);
char*    manta_abi_submit_form(uint32_t tab_id, uint32_t form_or_control_node_id);
uint8_t* manta_abi_submit_form_bytes(uint32_t tab_id, uint32_t form_or_control_node_id, uint32_t* out_len);
bool     manta_abi_save_form_draft(const char* origin_url, const char* field_key, const char* value);
char*    manta_abi_get_form_draft(const char* origin_url, const char* field_key);

/* Low-Latency Zero-Cache Media Streaming (HLS / DASH / Progressive Range) */
uint32_t manta_abi_media_open_stream(const char* source_url, uint32_t ring_bytes_pow2);
bool     manta_abi_media_ingest_manifest(uint32_t stream_id, const char* base_url, const char* manifest_text);
char*    manta_abi_media_next_prefetch_json(uint32_t stream_id);
uint32_t manta_abi_media_feed_bytes(uint32_t stream_id, const uint8_t* data_ptr, uint32_t data_len, uint32_t download_ms, uint32_t rtt_ms, uint32_t seg_dur_ms);
uint32_t manta_abi_media_read_chunk(uint32_t stream_id, uint8_t* out_buf, uint32_t max_len);
bool     manta_abi_media_seek_ms(uint32_t stream_id, uint64_t target_ms);
char*    manta_abi_media_get_stats_json(uint32_t stream_id);
bool     manta_abi_media_close_stream(uint32_t stream_id);

/* Encrypted WAL Storage, Tab Sessions, Offline Page Snapshots & Zero-Trace Mode */
bool     manta_abi_set_ephemeral_mode(bool enabled);
bool     manta_abi_set_vault_passphrase(const char* passphrase);
bool     manta_abi_storage_set(const char* origin_url, const char* key, const char* value, bool is_session);
char*    manta_abi_storage_get(const char* origin_url, const char* key, bool is_session);
bool     manta_abi_storage_remove(const char* origin_url, const char* key, bool is_session);
bool     manta_abi_cookie_set(const char* origin_url, const char* raw_cookie_header);
char*    manta_abi_cookie_get(const char* origin_url);
bool     manta_abi_idb_put(const char* origin_url, const char* store_name, const char* key, const char* json_val);
char*    manta_abi_idb_get(const char* origin_url, const char* store_name, const char* key);
bool     manta_abi_idb_delete(const char* origin_url, const char* store_name, const char* key);
bool     manta_abi_vfs_save_file(const char* origin_url, const char* virtual_path, const char* mime_type, const uint8_t* data_ptr, uint32_t data_len);
bool     manta_abi_save_page_offline(const char* url, const char* html_text);
char*    manta_abi_load_page_offline(const char* url);
char*    manta_abi_list_saved_tabs_json(void);
char*    manta_abi_inspect_storage_manifest_json(void);
bool     manta_abi_clear_origin_data(const char* origin_url);
bool     manta_abi_shred_all_storage_and_ram(void);
bool     manta_abi_flush_storage(void);

/* Non-Blocking Resumable Downloads & File Saving */
uint32_t manta_abi_start_download(const char* url, const char* suggested_filename);
uint32_t manta_abi_save_data_to_downloads(const char* filename, const char* mime_type, const uint8_t* data_ptr, uint32_t data_len);
char*    manta_abi_list_downloads_json(void);
bool     manta_abi_delete_download(uint32_t download_id, bool delete_file_from_disk);

/* Settings & Browser Controls */
bool     manta_abi_set_setting(const char* key, const char* value);
char*    manta_abi_export_settings_json(void);

/* Memory Management */
void     manta_abi_free_string(char* ptr);
void     manta_abi_free_bytes(uint8_t* ptr, uint32_t len);

#ifdef __cplusplus
}
#endif

#endif /* MANTA_CORE_H */
