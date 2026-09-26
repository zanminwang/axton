//! Source `@default` compiles to create-only `createDefault` metadata
//! ([#27](https://github.com/zanminwang/axton/issues/27)).
use axton_compiler::compile;
use serde_json::{Value, json};

const TODO: &str = "enum Status { open closed }
model Todo {
  id String @default(uuid())
  title String @default(\"\")
  done Boolean @default(false)
  priority Int @default(0)
  status Status @default(open)
  createdAt DateTime @default(now())
  @@id(id)
}
mutation AddTodo(todo Todo.create) {}
";

fn field<'a>(config: &'a Value, model: &str, name: &str) -> &'a Value {
    config["schema"]["models"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["name"] == model)
        .unwrap()["fields"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["name"] == name)
        .unwrap()
}

#[test]
fn the_todo_example_compiles_to_tagged_create_defaults() {
    let config = compile(TODO).unwrap();
    let expect = [
        ("id", json!({"kind":"uuid"})),
        ("title", json!({"kind":"literal","value":""})),
        ("done", json!({"kind":"literal","value":false})),
        ("priority", json!({"kind":"literal","value":0})),
        ("status", json!({"kind":"literal","value":"open"})),
        ("createdAt", json!({"kind":"now"})),
    ];
    for (name, create_default) in expect {
        let f = field(&config, "Todo", name);
        assert_eq!(f["createDefault"], create_default, "{name}");
        // Source @default never feeds the internal migration default.
        assert!(f.get("default").is_none(), "{name}: {f}");
    }
    axton_core::Schema::from_value(config["schema"].clone()).unwrap();
    // Nothing is evaluated at compile time: the same source renders the same bytes.
    assert_eq!(compile(TODO).unwrap(), config);
}

#[test]
fn literals_are_normalized_by_their_field_contract() {
    let config = compile(
        "enum Mood { calm busy }
model Reading {
  id UUID @default(uuid())
  score Float @default(-1.5)
  weight Float @default(2)
  small Float @default(1e-3)
  note String? @default(\"q \\\"x\\\" \\\\ \\n $y ''' \\\"\\\"\\\"\")
  mood Mood? @default(busy)
  takenAt DateTime @default(\"2026-01-02T03:04:05+02:00\")
  ref UUID @default(\"01890F47-1234-7123-8123-123456789ABC\")
  label String @default(uuid())
  low Int @default(-9007199254740991)
  plain String?
  @@id(id)
}
",
    )
    .unwrap();
    let default = |name: &str| field(&config, "Reading", name)["createDefault"].clone();
    assert_eq!(default("id"), json!({"kind":"uuid"}));
    assert_eq!(default("score"), json!({"kind":"literal","value":-1.5}));
    assert_eq!(default("weight"), json!({"kind":"literal","value":2.0}));
    assert_eq!(default("small"), json!({"kind":"literal","value":0.001}));
    assert_eq!(
        default("note"),
        json!({"kind":"literal","value":"q \"x\" \\ \n $y ''' \"\"\""})
    );
    assert_eq!(default("mood"), json!({"kind":"literal","value":"busy"}));
    assert_eq!(
        default("takenAt"),
        json!({"kind":"literal","value":"2026-01-02T01:04:05.000Z"})
    );
    assert_eq!(
        default("ref"),
        json!({"kind":"literal","value":"01890f47-1234-7123-8123-123456789abc"})
    );
    assert_eq!(default("label"), json!({"kind":"uuid"}));
    assert_eq!(
        default("low"),
        json!({"kind":"literal","value":-9007199254740991i64})
    );
    assert_eq!(
        default("plain"),
        Value::Null,
        "no metadata without @default"
    );
    axton_core::Schema::from_value(config["schema"].clone()).unwrap();
}

#[test]
fn invalid_defaults_are_rejected_at_the_declaration() {
    let model = |field: &str| {
        format!("enum Status {{ open closed }}\nmodel A {{\n id UUID\n {field}\n @@id(id)\n}}\n")
    };
    let cases: Vec<(String, &str)> = vec![
        (model("n Int @default(1) @default(2)"), "duplicate field directive"),
        (model("tags String[] @default(\"a\")"), "@default is unsupported on list field tags"),
        (model("n Int @default(null)"), "@default(null) is unsupported"),
        (model("n Int? @default(null)"), "@default(null) is unsupported"),
        (model("n Int @default()"), "expected default value"),
        (model("n Int @default(\"1\")"), "invalid default for n"),
        (model("n Int @default(1.5)"), "invalid default for n"),
        (model("n Int @default(9007199254740992)"), "invalid default for n"),
        (model("b Boolean @default(1)"), "invalid default for b"),
        (model("s String @default(1)"), "invalid default for s"),
        (model("s String @default(word)"), "invalid default for s"),
        (model("f Float @default(\"1\")"), "invalid default for f"),
        (model("s Status @default(pending)"), "invalid default for s"),
        (model("s Status @default(\"open\")"), "invalid default for s"),
        (model("t DateTime @default(\"2026-13-01T00:00:00Z\")"), "invalid default for t"),
        (model("t DateTime @default(\"2026-01-01\")"), "invalid default for t"),
        (model("u UUID @default(\"not-a-uuid\")"), "invalid default for u"),
        (model("n Int @default(uuid())"), "uuid() requires a String or UUID field"),
        (model("t String @default(now())"), "now() requires a DateTime field"),
        (model("s String @default(cuid())"), "unknown default function cuid()"),
        (model("s String @default(uuid(4))"), "uuid() takes no arguments"),
        (model("t DateTime @default(now(\"utc\"))"), "now() takes no arguments"),
        (model("n Int @default(01)"), "invalid default number"),
        (
            "model P { id UUID @@id(id) }\nmodel A {\n id UUID\n pId UUID\n p P @reference(via: [pId]) @default(uuid())\n @@id(id)\n}\n".into(),
            "@default is unsupported on relation field p",
        ),
        (
            "model A { id UUID @@id(id) }\nmutation M(x String @default(\"a\"))".into(),
            "@default is a Model field attribute",
        ),
        (
            "model A { id UUID @@id(id) }\nmutation M(x String) { y String @default(\"a\") }".into(),
            "@default is a Model field attribute",
        ),
    ];
    for (source, expected) in cases {
        let err = compile(&source).unwrap_err();
        assert!(err.contains(expected), "{source}\n=> {err}");
        let line = err.split(':').next().unwrap().parse::<usize>().unwrap();
        assert!(line >= 2, "diagnostic names the declaration: {err}");
    }
}
