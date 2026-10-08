//! The Strava MCP against the TypeScript it is ported from.
//!
//! `fixtures/strava.json` holds what `app/services/builtin/strava` and
//! `app/validators/builtin_strava.ts` answer, run from those very files by
//! Node:
//!
//! - what each tool is: its description, the schema it advertises with its
//!   keys in order, the permission it needs, and the schema it enforces;
//! - for every argument of every tool, what some hundred and thirty values
//!   become or the sentence that refuses them;
//! - what is read from a body Strava answers with;
//! - how payloads are compacted, summarized and downsampled;
//! - some hundred and sixty calls with the answers Strava gave: the requests
//!   that were sent, down to their encoding, and the JSON or the sentence
//!   the agent got back.
//!
//! The script that wrote the fixture is not part of the repository, since it
//! runs Node on the TypeScript app, with `TZ=UTC`.

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};

use http::{Method, StatusCode};
use mymcps_builtin::{BuiltinEnv, BuiltinError, BuiltinToolContext};
use mymcps_core::TestCore;
use mymcps_net::{CannedResponse, Fetcher, SentRequest};
use mymcps_strava::payload::{activity_summaries, compact_strava_payload, downsample_streams};
use mymcps_strava::validators::{STRAVA_ATHLETE_VALIDATOR, STRAVA_FAILURE_VALIDATOR};
use serde_json::{Map, Value, json};

use crate::support::{STRAVA, provider};

const FIXTURE: &str = include_str!("../fixtures/strava.json");

fn fixture() -> Value {
    serde_json::from_str(FIXTURE).unwrap()
}

fn text(value: &Value) -> &str {
    value.as_str().unwrap()
}

fn list(value: &Value) -> &[Value] {
    value.as_array().unwrap()
}

fn context(core: &TestCore, fetcher: Fetcher) -> Arc<BuiltinToolContext> {
    Arc::new(BuiltinToolContext {
        env: BuiltinEnv::new(core.core.clone()).with_fetcher(fetcher),
        mcp_id: 1,
        access_token: "strava-access-token".to_owned(),
        granted_scopes: None,
        settings: BTreeMap::new(),
    })
}

/// Fails with every difference, or the first twenty of many.
#[track_caller]
fn assert_none(differences: &[String], compared: usize) {
    assert!(
        differences.is_empty(),
        "{} of {compared} differ:\n{}",
        differences.len(),
        differences
            .iter()
            .take(20)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn is_the_definition_the_typescript_registers() {
    let fixture = fixture();
    let expected = &fixture["definition"];
    let oauth = STRAVA.oauth().unwrap();

    assert_eq!(STRAVA.key(), text(&expected["key"]));
    assert_eq!(STRAVA.name(), text(&expected["name"]));
    let authorize_params: Vec<Value> = oauth
        .authorize_params
        .iter()
        .map(|(name, value)| json!([name, value]))
        .collect();
    assert_eq!(
        json!({
            "issuer": oauth.issuer,
            "authorizeUrl": oauth.authorize_url,
            "tokenUrl": oauth.token_url,
            "scopes": oauth.scopes,
            "writeScopes": oauth.write_scopes,
            "scopeSeparator": oauth.scope_separator,
            "authorizeParams": authorize_params,
            "sendsRedirectUriWithCode": oauth.sends_redirect_uri_with_code,
            "clientIdHint": oauth.client_id_hint,
        }),
        expected["oauth"]
    );
    let pattern = oauth.client_id_pattern.as_ref().unwrap();
    for case in list(&expected["clientIds"]) {
        assert_eq!(
            pattern.is_match(text(&case[0])),
            case[1].as_bool().unwrap(),
            "{}",
            case[0]
        );
    }
}

#[test]
fn advertises_and_enforces_what_the_typescript_tools_do() {
    let fixture = fixture();
    let expected = list(&fixture["tools"]);
    let tools = STRAVA.tools();
    let enforced = STRAVA.enforced_schemas();
    assert_eq!(expected.len(), 21);
    assert_eq!(tools.len(), expected.len());

    for ((tool, (enforced_name, enforced)), expected) in tools.iter().zip(&enforced).zip(expected) {
        let name = tool.name;
        assert_eq!(name, text(&expected["name"]));
        assert_eq!(*enforced_name, name);
        assert_eq!(tool.description, text(&expected["description"]), "{name}");
        // As text: the order of the keys is part of what an agent reads.
        assert_eq!(
            tool.input_schema.to_string(),
            text(&expected["inputSchema"]),
            "{name}"
        );
        assert_eq!(
            json!(tool.requires_any_scope),
            expected["requiresAnyScope"],
            "{name}"
        );
        assert_eq!(tool.write, expected["write"].as_bool().unwrap(), "{name}");
        assert_eq!(tool.asks_approval, expected["approval"] == "ask", "{name}");
        assert_eq!(enforced.to_string(), text(&expected["enforced"]), "{name}");
    }
}

#[tokio::test]
async fn takes_and_refuses_the_arguments_the_typescript_validators_do() {
    let fixture = fixture();
    let inputs = &fixture["inputs"];
    let pool = list(&inputs["pool"]);
    let outcomes = list(&inputs["outcomes"]);
    let core = TestCore::new().await;
    let context = context(&core, Fetcher::offline());

    // The input as JSON after `=`, or the refusal after `!`.
    let outcome = |tool: &str, arguments: &Value| {
        let tool = provider().tool(tool).unwrap();
        match tool.input.validate(arguments.clone(), &context) {
            Ok(input) => format!("={input}"),
            Err(sentence) => format!("!{sentence}"),
        }
    };
    let expected =
        |index: &Value| text(&outcomes[usize::try_from(index.as_u64().unwrap()).unwrap()]);
    let mut compared = 0;
    let mut differences = Vec::new();
    let mut compare = |tool: &str, arguments: &Value, index: &Value| {
        compared += 1;
        let answered = outcome(tool, arguments);
        if answered != expected(index) {
            differences.push(format!(
                "{tool} {arguments}\n    typescript: {}\n    rust: {answered}",
                expected(index)
            ));
        }
    };

    for case in list(&inputs["whole"]) {
        compare(text(&case[0]), &case[1], &case[2]);
    }
    for (tool, by_argument) in inputs["byArgument"].as_object().unwrap() {
        let base = inputs["base"][tool].as_object().unwrap();
        for (argument, indexes) in by_argument.as_object().unwrap() {
            let indexes = list(indexes);
            assert_eq!(indexes.len(), pool.len(), "{tool} {argument}");
            for (value, index) in pool.iter().zip(indexes) {
                let mut arguments: Map<String, Value> = base.clone();
                match list(value).first() {
                    Some(value) => arguments.insert(argument.clone(), value.clone()),
                    None => arguments.shift_remove(argument),
                };
                compare(tool, &Value::Object(arguments), index);
            }
        }
    }

    assert!(compared > 9_000, "{compared}");
    assert_none(&differences, compared);
}

#[test]
fn reads_from_a_body_what_the_typescript_validators_read() {
    let fixture = fixture();
    let read = |validator: &mymcps_vine::Validator, body: &Value| {
        validator
            .try_validate(body)
            .ok()
            .map(|output| Value::String(output.to_string()))
            .unwrap_or(Value::Null)
    };

    let answers = list(&fixture["answers"]);
    assert_eq!(answers.len(), 28);
    for answer in answers {
        let body = &answer["body"];
        assert_eq!(
            read(&STRAVA_ATHLETE_VALIDATOR, body),
            answer["athlete"],
            "{body}"
        );
        assert_eq!(
            read(&STRAVA_FAILURE_VALIDATOR, body),
            answer["failure"],
            "{body}"
        );
    }
}

#[test]
fn shapes_payloads_as_the_typescript_does() {
    let fixture = fixture();
    let payloads = &fixture["payloads"];
    let mut compared = 0;
    let mut differences = Vec::new();
    let mut compare = |what: &str, case: &Value, shaped: Value| {
        compared += 1;
        let shaped = shaped.to_string();
        if shaped != text(&case["result"]) {
            differences.push(format!(
                "{what} {}\n    typescript: {}\n    rust: {shaped}",
                case["value"],
                text(&case["result"])
            ));
        }
    };

    for case in list(&payloads["compact"]) {
        compare(
            "compact",
            case,
            compact_strava_payload(case["value"].clone()),
        );
    }
    for case in list(&payloads["summaries"]) {
        compare("summaries", case, activity_summaries(case["value"].clone()));
    }
    for case in list(&payloads["streams"]) {
        let max_points = usize::try_from(case["maxPoints"].as_u64().unwrap()).unwrap();
        compare(
            "streams",
            case,
            downsample_streams(&case["value"], max_points),
        );
    }

    assert!(compared > 200, "{compared}");
    assert_none(&differences, compared);
}

/// A request as the script recorded it.
fn recorded(request: &SentRequest) -> Value {
    json!({
        "method": request.method.as_str(),
        "url": request.url.as_str(),
        "accept": request.header("accept"),
        "authorization": request.header("authorization"),
        "contentType": request.header("content-type"),
        "body": if request.method == Method::GET { String::new() } else { request.text() },
    })
}

fn canned(answer: &Value) -> CannedResponse {
    let status = u16::try_from(answer["status"].as_u64().unwrap()).unwrap();
    let mut response = CannedResponse::new(StatusCode::from_u16(status).unwrap())
        .body(text(&answer["body"]).to_owned());
    for (name, value) in answer["headers"].as_object().unwrap() {
        response = response.header(name, text(value)).unwrap();
    }
    response
}

#[tokio::test]
async fn calls_strava_and_answers_the_agent_as_the_typescript_does() {
    let fixture = fixture();
    let calls = list(&fixture["calls"]);
    let core = TestCore::new().await;
    let mut differences = Vec::new();

    for call in calls {
        let answers: VecDeque<CannedResponse> = list(&call["answers"]).iter().map(canned).collect();
        let answers = Mutex::new(answers);
        let requests: Arc<Mutex<Vec<Value>>> = Arc::default();
        let fetcher = Fetcher::offline().answering({
            let requests = Arc::clone(&requests);
            move |request| {
                requests.lock().unwrap().push(recorded(request));
                answers.lock().unwrap().pop_front()
            }
        });

        let tool = provider().tool(text(&call["tool"])).unwrap();
        let arguments = call["arguments"].as_object().unwrap().clone();
        let outcome = match tool.run(arguments, context(&core, fetcher)).await {
            Ok(result) => json!({ "result": result.to_string() }),
            Err(BuiltinError::Authorization(message)) => {
                json!({ "error": { "kind": "authorization", "message": message } })
            }
            Err(BuiltinError::Tool(message)) => {
                json!({ "error": { "kind": "tool", "message": message } })
            }
            // Not a sentence for the agent: only that it is none is compared.
            Err(BuiltinError::Internal(_)) => json!({ "error": { "kind": "other" } }),
        };

        let mut expected = json!({});
        if let Some(result) = call.get("result") {
            expected["result"] = result.clone();
        }
        if let Some(error) = call.get("error") {
            expected["error"] = error.clone();
            if error["kind"] == "other" {
                expected["error"] = json!({ "kind": "other" });
            }
        }
        let sent = Value::Array(requests.lock().unwrap().clone());
        if outcome != expected || sent != call["requests"] {
            differences.push(format!(
                "{} {}\n    typescript: {expected} after {}\n    rust: {outcome} after {sent}",
                call["tool"], call["arguments"], call["requests"]
            ));
        }
    }

    assert!(calls.len() > 150, "{}", calls.len());
    assert_none(&differences, calls.len());
}
