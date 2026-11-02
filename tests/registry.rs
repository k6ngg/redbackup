use redbackup::{
    config::{registry, settings::Settings},
    repository::initialize_repository,
};

#[test]
fn concurrent_registration_preserves_entries_and_corrupt_registry() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("config");
    let mut roots = Vec::new();
    for n in 0..4 {
        let root =
            initialize_repository(tmp.path().join(format!("repo{n}")).to_str().unwrap()).unwrap();
        Settings {
            source_path: "/source".into(),
            destination_path: root.display().to_string(),
            ..Settings::default()
        }
        .save(&root)
        .unwrap();
        roots.push(root);
    }
    std::thread::scope(|scope| {
        for root in &roots {
            let dir = &dir;
            scope.spawn(move || {
                registry::update_at(dir, root, true).unwrap();
                registry::update_at(dir, root, true).unwrap();
            });
        }
    });
    assert_eq!(registry::list_at(&dir).unwrap().len(), 4);
    registry::update_at(&dir, &roots[0], false).unwrap();
    assert!(roots[0].join("metadata/index.db").exists());
    assert_eq!(registry::list_at(&dir).unwrap().len(), 3);
    std::fs::write(dir.join("repositories.json"), b"invalid").unwrap();
    assert!(registry::update_at(&dir, &roots[0], true).is_err());
    assert_eq!(
        std::fs::read(dir.join("repositories.json")).unwrap(),
        b"invalid"
    );
}
