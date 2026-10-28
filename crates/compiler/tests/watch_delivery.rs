use axton_compiler::{compile, dart, typescript};

#[test]
fn generated_watches_pass_row_decoders_and_snapshot_copiers_to_the_sdk() {
    let schema =
        compile("model Entry { id String at DateTime tags String[] dates DateTime[] @@id(id) }")
            .unwrap();
    let ts = typescript(&schema);
    assert!(ts.contains("this.port.watchDecoded('Entry'"), "{ts}");
    assert!(!ts.contains("listener(rows.map(decodeEntry))"));
    let dart = dart(&schema);
    assert!(
        dart.contains("client.watchDecoded<Entry>('Entry'"),
        "{dart}"
    );
    assert!(!dart.contains("rows.map(Entry.fromRecord)"));
}

#[test]
fn scalar_only_dart_models_can_be_shared_but_mutable_fields_need_copies() {
    let schema = compile("model Plain { id String at DateTime @@id(id) } model Entry { id String tags String[] @@id(id) }").unwrap();
    let dart = dart(&schema);
    assert!(
        dart.contains("Plain _copyPlainWatch(Plain value) => value;"),
        "{dart}"
    );
    assert!(
        dart.contains("tags: value.tags.map((e) => e).toList()"),
        "{dart}"
    );
    let ts = typescript(&schema);
    assert!(ts.contains("new Date(value.at.getTime())"), "{ts}");
}
