#ifndef MANTA_CORE_H
#define MANTA_CORE_H

#include <stdint.h>
#include <stdbool.h>

#ifdef __cplusplus
extern "C" {
#endif

/**
 * Manta Core v2.5 — Universal Bolt-On C-ABI Header
 * High-velocity Rust Browser Engine with Transactional MantaDB3 Storage,
 * Custom High-Level Kernel Scripts, 99% Web Compatibility, and Non-Destructive Active Tabs.
 */

uint32_t manta_abi_init_engine(const char* storage_path, const char* download_dir);
uint32_t manta_abi_get_worker_cores(void);

/* Non-Destructive Hot-Active Tab Multiplexing (Tabs never killed while open) */
uint32_t manta_abi_create_tab(void);
bool     manta_abi_switch_tab(uint32_t tab_id);
bool     manta_abi_close_tab(uint32_t tab_id);
uint32_t manta_abi_active_tab_id(void);
uint32_t manta_abi_open_tab_count(void);

/* High-Velocity Kernel Script & Web Compatibility Compiler */
char*    manta_abi_compile_kernel_script(const char* origin_url);

/* Streaming HTML/CSS/JS & DisplayList Frame Output */
char*    manta_abi_stream_html_chunk(uint32_t tab_id, const char* url, const char* html_chunk, bool is_eof);

/* Interactive Forms, Inputs, Auto-Saved Drafts & File Upload */
bool     manta_abi_focus_node(uint32_t tab_id, uint32_t node_id);
bool     manta_abi_set_input_value(uint32_t tab_id, uint32_t node_id, const char* value);
bool     manta_abi_toggle_checkable(uint32_t tab_id, uint32_t node_id);
bool     manta_abi_attach_file_to_input(uint32_t tab_id, uint32_t node_id, const char* filename, const char* mime_type, const uint8_t* data_ptr, uint32_t data_len);
char*    manta_abi_submit_form(uint32_t tab_id, uint32_t form_or_control_node_id);
bool     manta_abi_save_form_draft(const char* origin_url, const char* field_key, const char* value);
char*    manta_abi_get_form_draft(const char* origin_url, const char* field_key);

/* Transactional MantaDB3, IndexedDB, Cookies, Virtual File System (OPFS) & Clearable Storage */
bool     manta_abi_storage_set(const char* origin_url, const char* key, const char* value, bool is_session);
char*    manta_abi_storage_get(const char* origin_url, const char* key, bool is_session);
bool     manta_abi_storage_remove(const char* origin_url, const char* key, bool is_session);
bool     manta_abi_cookie_set(const char* origin_url, const char* raw_cookie_header);
char*    manta_abi_cookie_get(const char* origin_url);
bool     manta_abi_idb_put(const char* origin_url, const char* store_name, const char* key, const char* json_val);
char*    manta_abi_idb_get(const char* origin_url, const char* store_name, const char* key);
bool     manta_abi_idb_delete(const char* origin_url, const char* store_name, const char* key);
bool     manta_abi_vfs_save_file(const char* origin_url, const char* virtual_path, const char* mime_type, const uint8_t* data_ptr, uint32_t data_len);
char*    manta_abi_inspect_storage_manifest_json(void);
bool     manta_abi_clear_origin_data(const char* origin_url);
bool     manta_abi_shred_all_storage_and_ram(void);
bool     manta_abi_flush_storage(void);

/* Resumable Downloads & File Saving */
uint32_t manta_abi_start_download(const char* url, const char* suggested_filename);
uint32_t manta_abi_save_data_to_downloads(const char* filename, const char* mime_type, const uint8_t* data_ptr, uint32_t data_len);
char*    manta_abi_list_downloads_json(void);
bool     manta_abi_delete_download(uint32_t download_id, bool delete_file_from_disk);

/* Settings & Browser Controls */
bool     manta_abi_set_setting(const char* key, const char* value);
char*    manta_abi_export_settings_json(void);

/* Memory Management for strings returned by `manta_abi_*` */
void     manta_abi_free_string(char* ptr);

#ifdef __cplusplus
}
#endif

#endif /* MANTA_CORE_H */
