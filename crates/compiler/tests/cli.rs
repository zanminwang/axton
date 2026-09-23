use std::{fs, process::Command};
#[test]
fn cli_retains_history_and_does_not_overwrite_on_break() {
    let root = std::env::temp_dir().join(format!("axton-compiler-cli-{}", std::process::id()));
    let input = root.join("input");
    let out = root.join("out");
    fs::create_dir_all(&input).unwrap();
    let model = input.join("test.model");
    fs::write(
        &model,
        "model A { id UUID title String @@id(id) } mutation Save { a A.create }",
    )
    .unwrap();
    let compile = || {
        Command::new(env!("CARGO_BIN_EXE_axton"))
            .arg("compile")
            .arg(&input)
            .arg(&out)
            .output()
            .unwrap()
    };
    assert!(compile().status.success());
    let previous = fs::read(out.join("schema.json")).unwrap();
    fs::write(
        &model,
        "model A { id UUID @@id(id) } mutation Save { a A.create }",
    )
    .unwrap();
    let rejected = compile();
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("schema fence"));
    assert_eq!(fs::read(out.join("schema.json")).unwrap(), previous);
    // A required field is a new input contract for the create and a new read
    // contract for the model, so both versions move.
    fs::write(&model,"model A { id UUID title String count Int @@id(id) @@version(2) } mutation Save { a A.create @@version(2) }").unwrap();
    assert!(compile().status.success());
    let backend: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("backend.json")).unwrap()).unwrap();
    assert_eq!(backend["mutations"].as_array().unwrap().len(), 2);
    assert_eq!(
        backend["schema"]["clientPolicies"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let history: serde_json::Value =
        serde_json::from_slice(&fs::read(input.join("history").join("mutations.json")).unwrap())
            .unwrap();
    assert_eq!(
        history["mutations"]["Save"]
            .as_object()
            .unwrap()
            .keys()
            .collect::<Vec<_>>(),
        ["1", "2"]
    );
    assert!(
        !out.join("mutation-history.json").exists(),
        "history is kept beside the schema, not with generated output"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn cli_reads_the_superseded_history_location_and_writes_the_new_default() {
    let (root, input) = workspace("relocation");
    let out = root.join("out");
    let model = input.join("test.model");
    fs::write(
        &model,
        "model A { id UUID title String @@id(id) } mutation Save { a A.create }",
    )
    .unwrap();
    assert!(
        axton(&[input.as_os_str(), out.as_os_str()])
            .status
            .success()
    );
    // Recreate the superseded layout: history in the output directory, nothing beside the schema.
    let superseded = out.join("mutation-history.json");
    let default = input.join("history").join("mutations.json");
    fs::copy(&default, &superseded).unwrap();
    let retained = fs::read(&superseded).unwrap();
    fs::remove_dir_all(input.join("history")).unwrap();
    fs::write(
        &model,
        "model A { id UUID title String count Int @@id(id) } mutation Save { a A.create @@version(2) }",
    )
    .unwrap();
    let moved = axton(&[input.as_os_str(), out.as_os_str()]);
    assert!(moved.status.success());
    let notice = String::from_utf8_lossy(&moved.stderr);
    assert!(
        notice.contains(&format!("{}", superseded.display())),
        "{notice}"
    );
    assert!(
        notice.contains(&format!("{}", default.display())),
        "{notice}"
    );
    assert_eq!(notice.lines().count(), 1, "{notice}");
    let history: serde_json::Value = serde_json::from_slice(&fs::read(&default).unwrap()).unwrap();
    assert_eq!(
        history["mutations"]["Save"]
            .as_object()
            .unwrap()
            .keys()
            .collect::<Vec<_>>(),
        ["1", "2"],
        "the retained version survives the move"
    );
    assert_eq!(
        fs::read(&superseded).unwrap(),
        retained,
        "the old file is left in place, unchanged"
    );
    // A second run reads the new default and no longer reports a move.
    let again = axton(&[input.as_os_str(), out.as_os_str()]);
    assert!(again.status.success());
    assert_eq!(String::from_utf8_lossy(&again.stderr), "");
    let refused = axton(&[
        input.as_os_str(),
        out.as_os_str(),
        "--initialize-mutation-history".as_ref(),
    ]);
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("already exists"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn cli_initializes_the_history_at_the_new_default_when_neither_location_exists() {
    let (root, input) = workspace("initial-history");
    let out = root.join("out");
    fs::write(
        input.join("test.model"),
        "model A { id UUID title String @@id(id) } mutation Save { a A.create }",
    )
    .unwrap();
    let first = axton(&[input.as_os_str(), out.as_os_str()]);
    assert!(first.status.success());
    assert_eq!(String::from_utf8_lossy(&first.stderr), "");
    let history: serde_json::Value =
        serde_json::from_slice(&fs::read(input.join("history").join("mutations.json")).unwrap())
            .unwrap();
    assert_eq!(history["formatVersion"], 1);
    assert_eq!(history["mutations"]["Save"]["1"]["version"], 1);
    assert!(!out.join("mutation-history.json").exists());
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn cli_writes_backend_ts_with_the_requested_runtime_import() {
    let root = std::env::temp_dir().join(format!("axton-compiler-backend-{}", std::process::id()));
    let input = root.join("input");
    let out = root.join("out");
    fs::create_dir_all(&input).unwrap();
    fs::write(
        input.join("test.model"),
        "model A { id UUID title String @@id(id) } mutation Save { a A.create }",
    )
    .unwrap();
    let status = Command::new(env!("CARGO_BIN_EXE_axton"))
        .arg("compile")
        .arg(&input)
        .arg(&out)
        .arg("--backend-runtime")
        .arg("../../packages/server/index.mts")
        .arg("--client-runtime")
        .arg("../../packages/client-js/index.mts")
        .status()
        .unwrap();
    assert!(status.success());
    let backend = fs::read_to_string(out.join("backend.ts")).unwrap();
    assert!(backend.contains("from \"../../packages/server/index.mts\""));
    assert!(backend.contains(" save: { v1(call: HandlerCall<Tx, SaveInput>)"));
    assert!(backend.contains(" a: { v1(call: LoaderCall<Tx, AIdentity>)"));
    let client = fs::read_to_string(out.join("client.ts")).unwrap();
    assert!(client.contains("from \"../../packages/client-js/index.mts\""));
    assert!(client.contains(" static async open(options: { path: string;"));
    assert!(!client.contains("owner"));
    fs::remove_dir_all(root).unwrap();
}

fn workspace(tag: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    let root = std::env::temp_dir().join(format!("axton-compiler-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let input = root.join("input");
    fs::create_dir_all(&input).unwrap();
    (root, input)
}

fn axton(args: &[&std::ffi::OsStr]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_axton"))
        .arg("compile")
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn cli_relocates_errors_into_the_file_that_declares_them() {
    let (root, input) = workspace("relocate");
    let out = root.join("out");
    fs::write(
        input.join("a.model"),
        "model Parent {\n id UUID\n @@id(id)\n}\n",
    )
    .unwrap();
    fs::write(
        input.join("b.model"),
        "model Child {\n id UUID\n parent Parent @reference(via: [missing])\n @@id(id)\n}\n",
    )
    .unwrap();
    let rejected = axton(&[input.as_os_str(), out.as_os_str()]);
    assert!(!rejected.status.success());
    let stderr = String::from_utf8_lossy(&rejected.stderr);
    assert!(
        stderr.contains(&format!("{}:3:", input.join("b.model").display())),
        "{stderr}"
    );
    assert!(stderr.contains("unknown reference field"), "{stderr}");
    fs::write(
        input.join("b.model"),
        "model Child {\n id UUID\n @@id(id)\n}\nbogus Stuff {}\n",
    )
    .unwrap();
    let rejected = axton(&[input.as_os_str(), out.as_os_str()]);
    let stderr = String::from_utf8_lossy(&rejected.stderr);
    assert!(
        stderr.contains(&format!("{}:5:", input.join("b.model").display())),
        "{stderr}"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn cli_output_is_deterministic() {
    let (root, input) = workspace("determinism");
    fs::copy(
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/compiler/relations.model"
        ),
        input.join("relations.model"),
    )
    .unwrap();
    let first = root.join("first");
    let second = root.join("second");
    assert!(
        axton(&[input.as_os_str(), first.as_os_str()])
            .status
            .success()
    );
    assert!(
        axton(&[input.as_os_str(), second.as_os_str()])
            .status
            .success()
    );
    let mut names: Vec<_> = fs::read_dir(&first)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    names.sort();
    assert_eq!(names.len(), 6, "{names:?}");
    assert!(input.join("history").join("mutations.json").exists());
    assert!(input.join("history").join("models.json").exists());
    assert_eq!(
        fs::read(input.join("history").join("models.json")).unwrap(),
        {
            fs::remove_file(input.join("history").join("models.json")).unwrap();
            assert!(
                axton(&[input.as_os_str(), first.as_os_str()])
                    .status
                    .success()
            );
            fs::read(input.join("history").join("models.json")).unwrap()
        },
        "a regenerated model history is byte-identical"
    );
    for name in names {
        assert_eq!(
            fs::read(first.join(&name)).unwrap(),
            fs::read(second.join(&name)).unwrap(),
            "{name:?} differs between runs"
        );
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn cli_refuses_misuse_of_the_mutation_history() {
    let (root, input) = workspace("history");
    let out = root.join("out");
    fs::write(
        input.join("test.model"),
        "model A { id UUID @@id(id) } mutation Save { a A.create @@version(2) }",
    )
    .unwrap();
    let history = root.join("history.json");
    let missing = axton(&[
        input.as_os_str(),
        out.as_os_str(),
        "--mutation-history".as_ref(),
        history.as_os_str(),
    ]);
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("missing mutation history"));
    assert!(!out.exists(), "a refused compile must not write outputs");
    let not_first = axton(&[
        input.as_os_str(),
        out.as_os_str(),
        "--mutation-history".as_ref(),
        history.as_os_str(),
        "--initialize-mutation-history".as_ref(),
    ]);
    assert!(!not_first.status.success());
    assert!(String::from_utf8_lossy(&not_first.stderr).contains("begin at version 1"));
    fs::write(
        input.join("test.model"),
        "model A { id UUID @@id(id) } mutation Save { a A.create }",
    )
    .unwrap();
    assert!(
        axton(&[
            input.as_os_str(),
            out.as_os_str(),
            "--mutation-history".as_ref(),
            history.as_os_str(),
            "--initialize-mutation-history".as_ref(),
        ])
        .status
        .success()
    );
    assert!(history.exists());
    let again = axton(&[
        input.as_os_str(),
        out.as_os_str(),
        "--mutation-history".as_ref(),
        history.as_os_str(),
        "--initialize-mutation-history".as_ref(),
    ]);
    assert!(!again.status.success());
    assert!(String::from_utf8_lossy(&again.stderr).contains("already exists"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn cli_retains_model_history_and_refuses_a_breaking_read_change_without_a_bump() {
    let (root, input) = workspace("model-history");
    let out = root.join("out");
    let model = input.join("test.model");
    let mutations = input.join("history").join("mutations.json");
    let models = input.join("history").join("models.json");
    fs::write(
        &model,
        "enum Status { open closed } model Task { id UUID title String note String? status Status @@id(id) } mutation Save { t Task.update<title> }",
    )
    .unwrap();
    assert!(
        axton(&[input.as_os_str(), out.as_os_str()])
            .status
            .success()
    );
    let first: serde_json::Value = serde_json::from_slice(&fs::read(&models).unwrap()).unwrap();
    assert_eq!(first["formatVersion"], 1);
    assert_eq!(first["models"]["Task"]["1"]["version"], 1);
    assert_eq!(
        first["models"]["Task"]["1"]["enums"][0]["values"],
        serde_json::json!(["open", "closed"])
    );
    let schema: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("schema.json")).unwrap()).unwrap();
    assert_eq!(
        schema["models"][0]["version"], 1,
        "the client declares its model version"
    );
    let outputs = || -> Vec<(String, Vec<u8>)> {
        let mut files: Vec<_> = fs::read_dir(&out)
            .unwrap()
            .map(|e| e.unwrap().path())
            .map(|p| {
                (
                    p.file_name().unwrap().to_string_lossy().into_owned(),
                    fs::read(&p).unwrap(),
                )
            })
            .collect();
        files.sort();
        files.push(("mutations.json".into(), fs::read(&mutations).unwrap()));
        files.push(("models.json".into(), fs::read(&models).unwrap()));
        files
    };
    let before = outputs();
    // A breaking read change (a new enum value) and a compatible mutation change in one
    // edit: the compile is refused as a whole, and neither history moves.
    fs::write(
        &model,
        "enum Status { open closed archived } model Task { id UUID title String note String? status Status @@id(id) } mutation Save { t Task.update<title> } mutation Rename { t Task.update<status> }",
    )
    .unwrap();
    let refused = axton(&[input.as_os_str(), out.as_os_str()]);
    assert!(!refused.status.success());
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert!(stderr.contains("Task v1"), "{stderr}");
    assert!(stderr.contains("enum \"Status\""), "{stderr}");
    assert!(stderr.contains("increase @@version"), "{stderr}");
    assert_eq!(
        outputs(),
        before,
        "a refused compile touches no output and no history"
    );
    // With the bump, the old contract stays as published beside the new one, and the fence
    // lets the bumped model drop a field.
    fs::write(
        &model,
        "enum Status { open closed archived } model Task { id UUID title String status Status @@id(id) @@version(2) } mutation Save { t Task.update<title> } mutation Rename { t Task.update<status> }",
    )
    .unwrap();
    let accepted = axton(&[input.as_os_str(), out.as_os_str()]);
    assert!(
        accepted.status.success(),
        "{}",
        String::from_utf8_lossy(&accepted.stderr)
    );
    let second: serde_json::Value = serde_json::from_slice(&fs::read(&models).unwrap()).unwrap();
    assert_eq!(
        second["models"]["Task"]
            .as_object()
            .unwrap()
            .keys()
            .collect::<Vec<_>>(),
        ["1", "2"]
    );
    assert_eq!(second["models"]["Task"]["1"], first["models"]["Task"]["1"]);
    assert_eq!(
        second["models"]["Task"]["2"]["enums"][0]["values"],
        serde_json::json!(["open", "closed", "archived"])
    );
    let backend: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("backend.json")).unwrap()).unwrap();
    let retained: Vec<(String, u64)> = backend["models"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| {
            (
                m["name"].as_str().unwrap().into(),
                m["version"].as_u64().unwrap(),
            )
        })
        .collect();
    assert_eq!(retained, [("Task".to_string(), 1), ("Task".to_string(), 2)]);
    assert_eq!(backend["models"][0], first["models"]["Task"]["1"]);
    assert_eq!(backend["schema"]["models"][0]["version"], 2);
    let history: serde_json::Value =
        serde_json::from_slice(&fs::read(&mutations).unwrap()).unwrap();
    assert!(history["mutations"]["Rename"]["1"].is_object());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn cli_refuses_misuse_of_the_model_history() {
    let (root, input) = workspace("model-history-misuse");
    let out = root.join("out");
    fs::write(
        input.join("test.model"),
        "model A { id UUID @@id(id) @@version(2) }",
    )
    .unwrap();
    let history = root.join("models.json");
    let missing = axton(&[
        input.as_os_str(),
        out.as_os_str(),
        "--model-history".as_ref(),
        history.as_os_str(),
    ]);
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("missing model history"));
    assert!(!out.exists(), "a refused compile must not write outputs");
    assert!(
        !input.join("history").exists(),
        "a refused compile must not write a history"
    );
    let not_first = axton(&[
        input.as_os_str(),
        out.as_os_str(),
        "--model-history".as_ref(),
        history.as_os_str(),
        "--initialize-model-history".as_ref(),
    ]);
    assert!(!not_first.status.success());
    assert!(
        String::from_utf8_lossy(&not_first.stderr)
            .contains("initial model history must begin at version 1")
    );
    fs::write(input.join("test.model"), "model A { id UUID @@id(id) }").unwrap();
    assert!(
        axton(&[
            input.as_os_str(),
            out.as_os_str(),
            "--model-history".as_ref(),
            history.as_os_str(),
            "--initialize-model-history".as_ref(),
        ])
        .status
        .success()
    );
    assert!(history.exists());
    assert!(
        !input.join("history").join("models.json").exists(),
        "an explicit path replaces the default"
    );
    assert!(
        input.join("history").join("mutations.json").exists(),
        "the mutation history keeps its default"
    );
    let again = axton(&[
        input.as_os_str(),
        out.as_os_str(),
        "--model-history".as_ref(),
        history.as_os_str(),
        "--initialize-model-history".as_ref(),
    ]);
    assert!(!again.status.success());
    assert!(String::from_utf8_lossy(&again.stderr).contains("model history already exists"));
    fs::remove_dir_all(root).unwrap();
}
