use db::AuthorityEffect;
use serde_json::{json, Value};

use super::*;

fn pins() -> Vec<String> {
    vec!["services-tests".to_owned()]
}

fn request_validation() -> Value {
    json!({"type":"request_validation","check_key":"services-tests",
        "subject":{"kind":"work_unit_result","id":"wu-1","version":7,"repo_id":"repo-1"},
        "baseline":true,"reason":"touches validation_service"})
}

fn ask_human() -> Value {
    json!({"type":"ask_human","question":"Waive the pre-existing failure?",
        "options":[
            {"id":"waive","label":"Waive","description":"Accept it.","consequence":"Gate may continue.","authority_effect":"scope"},
            {"id":"fix","label":"Fix first","description":"Open rework.","consequence":"One more cycle.","authority_effect":"none"}],
        "recommended_option_id":"fix","recommendation_rationale":"Touched code.",
        "evidence_links":[{"kind":"validation_comparison","id":"cmp-1","digest":"a".repeat(64)}]})
}

fn allow_all() -> Vec<CoordinatorOutputKind> {
    vec![
        CoordinatorOutputKind::RequestValidation,
        CoordinatorOutputKind::AskHuman,
        CoordinatorOutputKind::ProposeReplan,
    ]
}

#[test]
fn coordinator_replan_output_requires_explicit_permission_and_has_frozen_shape() {
    let value = json!({
        "type":"propose_replan",
        "reason":"The accepted split cannot be completed safely",
        "evidence_links":[{"kind":"artifact","id":"artifact-1","digest":"a".repeat(64)}],
        "plan_request":{"instructions":"Split the remaining accepted scope"}
    });
    let denied = parse_outputs(vec![value.clone()], &pins(), None).unwrap_err();
    assert_eq!(denied.code, CODE_OUTPUT_NOT_PERMITTED);
    let allowed = [CoordinatorOutputKind::ProposeReplan];
    let parsed = parse_outputs(vec![value], &pins(), Some(&allowed)).unwrap();
    assert!(matches!(
        parsed.as_slice(),
        [CoordinatorOutput::ProposeReplan(_)]
    ));
    let mut unknown = json!({
        "type":"propose_replan",
        "reason":"reason",
        "plan_request":{"instructions":"instructions","path":"src/secret.rs"}
    });
    assert!(CoordinatorOutput::from_value(std::mem::take(&mut unknown)).is_err());
}

#[test]
fn micro_fix_output_is_bounded_and_rejects_unowned_paths() {
    let valid = MicroFixOutput {
        finding_refs: vec!["finding-1".to_owned()],
        instructions: "Fix only this finding".to_owned(),
        paths: vec!["crates/services/src/lib.rs".to_owned()],
    };
    valid.validate().unwrap();
    assert!(MicroFixOutput {
        paths: vec!["../Cargo.toml".to_owned()],
        ..valid.clone()
    }
    .validate()
    .is_err());
    assert!(MicroFixOutput {
        paths: vec!["src/file\n.rs".to_owned()],
        ..valid.clone()
    }
    .validate()
    .is_err());
    assert!(MicroFixOutput {
        finding_refs: vec!["finding".to_owned(); MAX_MICRO_FIX_FINDINGS + 1],
        ..valid
    }
    .validate()
    .is_err());
}

#[test]
fn micro_fix_permission_is_never_implied_by_legacy_policy() {
    assert_eq!(
        ensure_new_kind_permitted(CoordinatorOutputKind::MicroFix, None)
            .unwrap_err()
            .code,
        CODE_OUTPUT_NOT_PERMITTED
    );
    let allowed = [CoordinatorOutputKind::MicroFix];
    ensure_new_kind_permitted(CoordinatorOutputKind::MicroFix, Some(&allowed)).unwrap();
}

#[test]
fn coordinator_outputs_parse_frozen_examples() {
    let allowed = allow_all();
    let outputs = parse_outputs(
        vec![request_validation(), ask_human()],
        &pins(),
        Some(&allowed),
    )
    .unwrap();
    assert_eq!(outputs.len(), 2);
    let CoordinatorOutput::AskHuman(ask) = &outputs[1] else {
        panic!("ask_human expected");
    };
    assert_eq!(ask.max_authority_effect(), AuthorityEffect::Scope);
    assert!(ask.options_json().unwrap().contains("\"waive\""));
}

#[test]
fn coordinator_outputs_reject_unknown_fields_and_commands() {
    let mut value = request_validation();
    value["command"] = json!("rm -rf /");
    assert!(CoordinatorOutput::from_value(value).is_err());
    let mut value = request_validation();
    value["subject"]["workspace_id"] = json!("w");
    assert!(CoordinatorOutput::from_value(value).is_err());
    let mut value = ask_human();
    value["options"][0]["extra"] = json!(1);
    assert!(CoordinatorOutput::from_value(value).is_err());
    assert!(CoordinatorOutput::from_value(json!({"type":"propose_replan"})).is_err());
}

#[test]
fn coordinator_outputs_new_kinds_require_explicit_allow() {
    for allowed in [None, Some(Vec::<CoordinatorOutputKind>::new())] {
        let error =
            parse_outputs(vec![request_validation()], &pins(), allowed.as_deref()).unwrap_err();
        assert_eq!(error.code, CODE_OUTPUT_NOT_PERMITTED);
    }
    let only_ask = [CoordinatorOutputKind::AskHuman];
    let error = parse_outputs(vec![request_validation()], &pins(), Some(&only_ask)).unwrap_err();
    assert_eq!(error.code, CODE_OUTPUT_NOT_PERMITTED);
    parse_outputs(vec![ask_human()], &pins(), Some(&only_ask)).unwrap();
}

#[test]
fn coordinator_outputs_check_must_be_pinned() {
    let allowed = allow_all();
    let none: Vec<String> = Vec::new();
    let error = parse_outputs(vec![request_validation()], &none, Some(&allowed)).unwrap_err();
    assert_eq!(error.code, CODE_CHECK_NOT_AUTHORIZED);
}

#[test]
fn coordinator_outputs_bound_turn_count() {
    let allowed = allow_all();
    let values = vec![ask_human(); MAX_NEW_OUTPUTS_PER_TURN + 1];
    let error = parse_outputs(values, &pins(), Some(&allowed)).unwrap_err();
    assert_eq!(error.code, CODE_INVALID_OUTPUT);
    parse_outputs(
        vec![ask_human(); MAX_NEW_OUTPUTS_PER_TURN],
        &pins(),
        Some(&allowed),
    )
    .unwrap();
}

#[test]
fn coordinator_outputs_request_validation_bounds() {
    let cases: Vec<fn(&mut Value)> = vec![
        |v| v["check_key"] = json!("Bad Key"),
        |v| v["check_key"] = json!(""),
        |v| v["subject"]["version"] = json!(0),
        |v| v["subject"]["id"] = json!(" "),
        |v| v["subject"]["repo_id"] = json!(" "),
        |v| v["reason"] = json!("x".repeat(MAX_REASON_BYTES + 1)),
        |v| v["reason"] = json!(""),
        |v| v["subject"]["kind"] = json!("workspace"),
    ];
    for mutate in cases {
        let mut value = request_validation();
        mutate(&mut value);
        assert!(CoordinatorOutput::from_value(value).is_err());
    }
    let mut value = request_validation();
    value.as_object_mut().unwrap().remove("baseline");
    let CoordinatorOutput::RequestValidation(request) =
        CoordinatorOutput::from_value(value).unwrap()
    else {
        panic!();
    };
    assert!(!request.baseline);
}

#[test]
fn multirepo_result_validation_output_requires_repo_id() {
    let mut value = request_validation();
    value["subject"].as_object_mut().unwrap().remove("repo_id");
    assert!(CoordinatorOutput::from_value(value).is_err());

    let mut value = request_validation();
    value["subject"]["repo_id"] = json!("");
    assert!(CoordinatorOutput::from_value(value).is_err());

    let parsed = CoordinatorOutput::from_value(request_validation()).unwrap();
    let CoordinatorOutput::RequestValidation(output) = parsed else {
        panic!("request_validation expected");
    };
    assert_eq!(output.subject.repo_id, "repo-1");
}

#[test]
fn coordinator_outputs_ask_human_option_rules() {
    let cases: Vec<fn(&mut Value)> = vec![
        |v| v["options"] = json!([v["options"][0].clone()]),
        |v| v["options"][1]["id"] = json!("waive"),
        |v| v["options"][1]["id"] = json!("Bad Id"),
        |v| v["options"][1]["id"] = json!("x".repeat(33)),
        |v| v["recommended_option_id"] = json!("missing"),
        |v| v["recommended_option_id"] = Value::Null,
        |v| v["question"] = json!("q".repeat(MAX_QUESTION_BYTES + 1)),
        |v| v["options"][0]["label"] = json!(""),
        |v| v["options"][0]["description"] = json!("d".repeat(513)),
        |v| {
            v["options"][0]
                .as_object_mut()
                .unwrap()
                .remove("authority_effect");
        },
    ];
    for mutate in cases {
        let mut value = ask_human();
        mutate(&mut value);
        let error = CoordinatorOutput::from_value(value);
        assert!(error.is_err(), "case must be rejected");
    }
    let mut value = ask_human();
    let option = value["options"][0].clone();
    value["options"] = json!(vec![option; 7]);
    assert!(CoordinatorOutput::from_value(value).is_err());
}

#[test]
fn coordinator_outputs_ask_human_evidence_rules() {
    let cases: Vec<fn(&mut Value)> = vec![
        |v| v["evidence_links"][0]["kind"] = json!("log_file"),
        |v| v["evidence_links"][0]["digest"] = json!("ABC"),
        |v| v["evidence_links"][0]["digest"] = json!("A".repeat(64)),
        |v| v["evidence_links"][0]["id"] = json!(""),
        |v| v["evidence_links"][0]["path"] = json!("/etc/passwd"),
        |v| {
            let link = v["evidence_links"][0].clone();
            v["evidence_links"] = json!(vec![link; 17]);
        },
    ];
    for mutate in cases {
        let mut value = ask_human();
        mutate(&mut value);
        assert!(CoordinatorOutput::from_value(value).is_err());
    }
    let mut value = ask_human();
    value["evidence_links"][0]
        .as_object_mut()
        .unwrap()
        .remove("digest");
    CoordinatorOutput::from_value(value).unwrap();
}

#[test]
fn coordinator_outputs_error_maps_to_coordination_service_error() {
    let error: ServiceError = OutputError::new(CODE_OUTPUT_NOT_PERMITTED, "no").into();
    assert!(matches!(
        error,
        ServiceError::Coordination {
            code: CODE_OUTPUT_NOT_PERMITTED,
            ..
        }
    ));
}
