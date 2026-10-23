#[test]
fn channel_set_is_refused_before_writing_config() {
    // Never let this test write a config or reach the network if the guard is ever turned off.
    if !crate::update::FORK_BLOCKS_SELF_UPDATE {
        panic!("fork builds must block channel set");
    }

    let _guard = crate::config::test_config_env_lock().lock().unwrap();
    let dir =
        std::env::temp_dir().join(format!("herdr-channel-set-refused-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let existing = dir.join("config.toml");
    let original = "[update]\nchannel = \"stable\"\n";
    std::fs::write(&existing, original).unwrap();
    let missing = dir.join("missing").join("config.toml");

    std::env::set_var(crate::config::CONFIG_PATH_ENV_VAR, &existing);
    let existing_exit = super::channel_set(&["preview".to_string()]).unwrap();
    std::env::set_var(crate::config::CONFIG_PATH_ENV_VAR, &missing);
    let missing_exit = super::channel_set(&["stable".to_string()]).unwrap();
    std::env::remove_var(crate::config::CONFIG_PATH_ENV_VAR);

    let after = std::fs::read_to_string(&existing).unwrap();
    let missing_dir_created = dir.join("missing").exists();
    let _ = std::fs::remove_dir_all(&dir);

    assert_eq!(existing_exit, 1);
    assert_eq!(after, original);
    assert_eq!(missing_exit, 1);
    assert!(!missing_dir_created);
}
