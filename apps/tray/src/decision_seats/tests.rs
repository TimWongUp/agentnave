use super::*;

fn seat() -> Value {
    json!({"id": "s-1a2b3c", "kind": "custom", "role": "写代码", "description": "写代码",
        "provider": "codex", "model": "gpt-6.1-sol", "access": "write", "difficulty": "hard"})
}

fn document(seats: Vec<Value>) -> Value {
    json!({"version": 1, "seats": seats})
}

#[test]
fn what_the_fixtures_do_not_cover_is_refused_like_the_service_does() {
    assert!(validate(&document(vec![seat()])).is_ok());
    let with = |field: &str, value: Value| {
        let mut changed = seat();
        changed[field] = value;
        validate(&document(vec![changed]))
    };
    // A role is trimmed first, then held to 40 printable characters.
    assert!(with("role", json!(format!("  {}  ", "角".repeat(40)))).is_ok());
    for role in [
        json!("角".repeat(41)),
        json!("  "),
        json!("a\nb"),
        json!("a\u{200b}b"),
    ] {
        assert!(with("role", role.clone()).is_err(), "{role}");
    }
    // Fixed seat names and review roles belong to the service.
    for id in [
        "explorer-easy",
        "worker-frontend-hard",
        "image-generator",
        "adversarial-verifier",
    ] {
        assert!(with("id", json!(id)).is_err(), "{id}");
    }
    assert!(with("id", json!("worker")).is_ok());
    assert!(with("id", json!("a".repeat(128))).is_ok());
    assert!(with("effort", json!(3)).is_err());
    assert!(with("note", json!("x")).is_err());
    assert!(with("difficulty", json!("medium")).is_ok());
    // The file keeps to what the service reads: its own fields, 255 seats, the one decision model.
    let many = |count: usize| {
        document(
            (0..count)
                .map(|i| {
                    let mut numbered = seat();
                    numbered["id"] = json!(format!("s-{i}"));
                    numbered["role"] = json!(format!("专业{i}"));
                    numbered["description"] = json!(format!("专业{i}"));
                    numbered
                })
                .collect(),
        )
    };
    assert!(validate(&many(255)).is_ok() && validate(&many(256)).is_err());
    let mut file = document(vec![]);
    file["extra"] = json!(1);
    assert!(validate(&file).is_err());
    // A card is a kind and a role with one seat per difficulty; a group has one name and keeps
    // its access; the specialty text is bounded; an image seat is write-only with no difficulty.
    let group = |id: &str, role: &str, difficulty: &str| {
        let mut seat = seat();
        seat["id"] = json!(id);
        seat["kind"] = json!("explorer");
        seat["role"] = json!(role);
        seat["access"] = json!("read");
        seat["difficulty"] = json!(difficulty);
        seat
    };
    let two = |a: Value, b: Value| validate(&document(vec![a, b]));
    assert!(
        two(
            group("a", "通用探索", "easy"),
            group("b", "通用探索", "hard")
        )
        .is_ok()
    );
    assert!(
        two(
            group("a", "通用探索", "easy"),
            group("b", "通用探索", "easy")
        )
        .is_err()
    );
    assert!(two(group("a", "通用探索", "easy"), group("b", "探索", "hard")).is_err());
    let mut writes = group("a", "通用探索", "easy");
    writes["access"] = json!("write");
    assert!(validate(&document(vec![writes])).is_err());
    assert!(with("kind", json!("gardening")).is_err());
    assert!(with("kind", json!("worker_writing")).is_ok());
    assert!(with("description", json!("长".repeat(200))).is_ok());
    assert!(with("description", json!("长".repeat(201))).is_err());
    assert!(with("description", json!("  ")).is_err());
    assert!(with("tags", json!(["coding"])).is_err());
    let mut difficulty_less = seat();
    difficulty_less
        .as_object_mut()
        .unwrap()
        .remove("difficulty");
    assert!(validate(&document(vec![difficulty_less.clone()])).is_err());
    difficulty_less["kind"] = json!("image");
    assert!(validate(&document(vec![difficulty_less.clone()])).is_ok());
    let mut other = difficulty_less.clone();
    other["id"] = json!("s-other");
    assert!(two(difficulty_less.clone(), other.clone()).is_err());
    other["role"] = json!("写实");
    assert!(two(difficulty_less.clone(), other).is_ok());
    for (field, value) in [("difficulty", json!("easy")), ("access", json!("read"))] {
        let mut bad = difficulty_less.clone();
        bad[field] = value;
        assert!(validate(&document(vec![bad])).is_err(), "{field}");
    }
    // The boss switches live in orchestration.json now.
    for key in ["apps", "effective_apps"] {
        let mut file = document(vec![]);
        file[key] = json!({"claude": true});
        assert!(validate(&file).is_err(), "{key}");
    }
}

#[test]
fn the_page_copy_is_made_ready_and_a_saved_file_reads_back() {
    let dir = tempfile::tempdir().unwrap();
    let mut file = document(vec![{
        let mut padded = seat();
        padded["role"] = json!("  写代码 ");
        padded
    }]);
    assert!(read(dir.path()).unwrap().is_none());
    normalize(&mut file);
    save(dir.path(), &file).unwrap();
    let read = read(dir.path()).unwrap().unwrap();
    assert_eq!(read["seats"][0]["role"], "写代码");
    assert_eq!(read["decision_model"], "gemini-decision-1");
    // A damaged file is reported, not replaced by an empty catalog.
    std::fs::write(dir.path().join(FILE), "{broken").unwrap();
    assert!(self::read(dir.path()).is_err());
    assert_eq!(
        std::fs::read_to_string(dir.path().join(FILE)).unwrap(),
        "{broken"
    );
}
