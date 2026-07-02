//! Smoke test: verifies the helpers compile and the binary is reachable.
//! This file can be removed once real E2E tests are added.

mod helpers;

#[test]
fn binary_is_reachable() {
    let proj = helpers::TempProject::new(helpers::CONFIG_MINIMAL);
    let out = proj.list();
    let items = out.as_array().expect("list should return an array");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["slug"], "simple");
}
