use super::*;

#[test]
fn child_command_uses_its_private_directory_and_preserves_windows_system_paths() {
    let profile = tempfile::tempdir().unwrap();
    let command = command(profile.path()).unwrap();
    let command = command.as_std();
    assert_eq!(command.get_current_dir(), Some(profile.path()));
    let env: BTreeMap<_, _> = command.get_envs().collect();
    assert_eq!(
        env.get(std::ffi::OsStr::new("CC_SWITCH_PLUGIN_PROFILE")),
        Some(&Some(profile.path().as_os_str()))
    );
    #[cfg(windows)]
    for key in ["SystemDrive", "ProgramData", "ALLUSERSPROFILE"] {
        let parent_value = std::env::var_os(key).expect("Windows system path is available");
        assert_eq!(
            env.get(std::ffi::OsStr::new(key)),
            Some(&Some(parent_value.as_os_str())),
            "missing {key}"
        );
    }
}

#[tokio::test]
async fn pipe_frames_are_bounded_and_round_trip() {
    let (mut writer, mut reader) = tokio::io::duplex(1024);
    let request = FromChild::Request {
        seq: 7,
        method: "getContext".into(),
        value: Value::Null,
    };
    write_frame(&mut writer, &request, MAX_MESSAGE)
        .await
        .unwrap();
    let bytes = read_frame(&mut reader, MAX_MESSAGE).await.unwrap();
    assert!(matches!(
        serde_json::from_slice::<FromChild>(&bytes).unwrap(),
        FromChild::Request { seq: 7, .. }
    ));
    writer.write_u32(MAX_MESSAGE as u32 + 1).await.unwrap();
    assert_eq!(
        read_frame(&mut reader, MAX_MESSAGE).await.unwrap_err(),
        "PLUGIN_MESSAGE_TOO_LARGE"
    );
    assert!(serde_json::from_str::<FromChild>(
        r#"{"kind":"request","seq":1,"method":"getStorage","value":null,"pluginId":"other"}"#
    )
    .is_err());
}

#[tokio::test]
async fn stale_stop_cannot_kill_a_new_session_and_renderer_reload_cleans_all_sessions() {
    let mut receivers = Vec::new();
    for id in ["old", "new"] {
        let (stop, receiver) = watch::channel(false);
        let (control, _commands) = mpsc::channel(1);
        sessions().unwrap().insert(
            id.into(),
            Session {
                plugin_id: "test.runtime".into(),
                stop,
                control,
                status: Arc::new(Mutex::new(RuntimeStatus {
                    session_id: id.into(),
                    state: "starting",
                    pid: None,
                })),
            },
        );
        receivers.push(receiver);
    }
    stop_ui_plugin_runtime("old".into()).await.unwrap();
    assert!(*receivers[0].borrow());
    assert!(!*receivers[1].borrow());
    stop_all();
    assert!(*receivers[1].borrow());
    assert!(sessions().unwrap().is_empty());
}

#[test]
fn child_serves_only_its_snapshot_and_preserves_the_sandbox() {
    let boot = Boot {
        title: "test".into(),
        html: "<h1>snapshot</h1>".into(),
        context: Value::Null,
    };
    let response = child::response("/page", &boot);
    assert_eq!(response.headers()["Content-Security-Policy"], PAGE_CSP);
    assert_eq!(response.headers()["Cache-Control"], "no-store");
    let html = String::from_utf8(response.into_body()).unwrap();
    assert!(html.contains("healthReply"));
    assert!(html.contains("<h1>snapshot</h1>"));
    assert_eq!(child::response("/other/plugin", &boot).status(), 404);
    assert!(!PAGE_CSP.contains("allow-same-origin"));
    assert!(PAGE_CSP.contains("connect-src 'none'"));
}

#[cfg(unix)]
#[tokio::test]
async fn terminating_a_hung_process_is_independent_of_its_event_loop() {
    let mut command = Command::new("/bin/sh");
    command
        .args(["-c", "while :; do :; done"])
        .process_group(0)
        .kill_on_drop(true);
    let mut child = command.spawn().unwrap();
    let pid = child.id().unwrap();
    let tree = ProcessTree::attach(&child).unwrap();
    tokio::time::timeout(Duration::from_secs(3), terminate(&mut child, &tree))
        .await
        .unwrap();
    assert!(child.try_wait().unwrap().is_some());
    assert_eq!(unsafe { libc::kill(pid as i32, 0) }, -1);
}
