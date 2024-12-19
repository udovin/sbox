use common::TempCgroup;

mod common;

#[test]
fn test_cgroup() {
    let cgroup = TempCgroup::new().unwrap();
    {
        let controllers = cgroup.subtree_controllers().unwrap();
        assert!(controllers.is_empty(), "{controllers:#?}");
    }
    cgroup
        .add_subtree_controllers(vec!["cpu".into(), "memory".into(), "pids".into()])
        .unwrap();
    {
        let mut controllers = cgroup.subtree_controllers().unwrap();
        controllers.sort();
        assert_eq!(controllers, ["cpu", "memory", "pids"]);
    }
}
