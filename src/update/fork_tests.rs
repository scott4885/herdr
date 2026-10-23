use super::{self_update, SelfUpdateOptions, FORK_BLOCKS_SELF_UPDATE};

#[test]
fn self_update_is_refused_for_fork_builds() {
    // Never let this test reach the network if the guard is ever turned off.
    if !FORK_BLOCKS_SELF_UPDATE {
        panic!("fork builds must block self-update");
    }

    let err = self_update(SelfUpdateOptions::default()).unwrap_err();

    assert_eq!(
        err,
        "self-update is disabled for this fork build; rebuild from the fork source, \
         or reinstall upstream Herdr deliberately with its installer"
    );
}
