use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::json;
use zagent_backend::fs::{MemoryFileSystem, RootedHostFileSystem};
use zagent_backend::tools::register_wasi_tools_with_filesystem;

#[tokio::test]
async fn wasi_tools_use_injected_memory_filesystem() {
    let fs = Arc::new(MemoryFileSystem::from_files([
        ("docs/readme.md", "# Title\nhello\n"),
        ("docs/nested/info.txt", "inside\n"),
    ]));
    let tools = register_wasi_tools_with_filesystem(fs);

    let listing = tools
        .execute(
            "list_dir",
            json!({
                "path": "docs",
                "recursive": true,
                "max_depth": 4
            }),
        )
        .await
        .expect("list_dir should succeed");
    assert!(listing.contains("nested/"), "{listing}");
    assert!(listing.contains("readme.md"), "{listing}");

    let readme = tools
        .execute("file_read", json!({ "path": "docs/readme.md" }))
        .await
        .expect("file_read should succeed");
    assert!(readme.contains("1 | # Title"), "{readme}");

    let write_result = tools
        .execute(
            "file_write",
            json!({
                "path": "docs/new.txt",
                "content": "created\n"
            }),
        )
        .await
        .expect("file_write should succeed");
    assert!(write_result.contains("docs/new.txt"), "{write_result}");

    let edit_result = tools
        .execute(
            "file_edit",
            json!({
                "path": "docs/new.txt",
                "diff": "@@ -1 +1 @@\n-created\n+updated\n"
            }),
        )
        .await
        .expect("file_edit should succeed");
    assert!(
        edit_result.contains("Successfully applied diff"),
        "{edit_result}"
    );

    let updated = tools
        .execute("file_read", json!({ "path": "docs/new.txt" }))
        .await
        .expect("file_read after edit should succeed");
    assert!(updated.contains("1 | updated"), "{updated}");
}

#[tokio::test]
async fn rooted_host_filesystem_rejects_path_escape() {
    let sandbox = temp_test_dir("sandbox");
    let outside = temp_test_dir("outside");

    std::fs::create_dir_all(&sandbox).expect("sandbox dir");
    std::fs::create_dir_all(&outside).expect("outside dir");
    std::fs::write(sandbox.join("visible.txt"), "visible\n").expect("visible file");
    std::fs::write(outside.join("secret.txt"), "secret\n").expect("secret file");

    let fs = Arc::new(RootedHostFileSystem::new(&sandbox).expect("rooted fs"));
    let tools = register_wasi_tools_with_filesystem(fs);

    let err = tools
        .execute("file_read", json!({ "path": "../outside/secret.txt" }))
        .await
        .expect_err("escaping root should fail");

    let message = err.to_string();
    assert!(message.contains("configured root"), "{message}");

    let visible = tools
        .execute("file_read", json!({ "path": "visible.txt" }))
        .await
        .expect("root-relative read should succeed");
    assert!(visible.contains("1 | visible"), "{visible}");

    let _ = std::fs::remove_dir_all(&sandbox);
    let _ = std::fs::remove_dir_all(&outside);
}

fn temp_test_dir(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic enough for tests")
        .as_nanos();
    std::env::temp_dir().join(format!("zagent-{label}-{nanos}"))
}
