use std::cell::RefCell;
use std::collections::VecDeque;
use serde_json::Value;

use super::*;

#[derive(Default)]
struct Fixture {
    replies: RefCell<VecDeque<Result<Value, crate::web_api::Error>>>,
    requests: RefCell<Vec<(Method, String, Option<Value>)>>,
}

impl Transport for Fixture {
    fn json(&self, method: Method, url: &str, body: Option<&Value>, auth: Option<Authorization<'_>>) -> Result<Value, crate::web_api::Error> {
        assert!(matches!(auth, Some(Authorization::Bearer(_))));
        self.requests.borrow_mut().push((method, url.into(), body.cloned()));
        self.replies.borrow_mut().pop_front().expect("unexpected request")
    }
}

fn backend(replies: impl IntoIterator<Item = Value>) -> Http<Fixture> {
    Http { transport: Fixture { replies: RefCell::new(replies.into_iter().map(Ok).collect()), ..Default::default() } }
}

fn key() -> Secret { Secret::new("private-fixture-token".into()).unwrap() }

#[test]
fn favourites_keep_their_order_units_and_server_names() {
    let backend = backend([
        json!({"entity_id":"sensor.temperature","state":"21.8","attributes":{"friendly_name":"Temperature","unit_of_measurement":"°C"}}),
        json!({"entity_id":"light.studio","state":"off","attributes":{"friendly_name":"Studio light"}}),
    ]);
    let favourites = ["sensor.temperature".into(), "light.studio".into(), "sensor.temperature".into()];
    let states = backend.states("https://home.example/proxy/", &key(), &favourites).unwrap();
    assert_eq!(states.iter().map(|entity| entity.id.as_str()).collect::<Vec<_>>(), ["sensor.temperature", "light.studio"]);
    assert_eq!(states[0].unit.as_deref(), Some("°C"));
    assert_eq!(states[1].name, "Studio light");
    assert_eq!(states[1].state, "off");
    assert_eq!(backend.transport.requests.borrow().as_slice(), [
        (Method::GET, "https://home.example/proxy/api/states/sensor.temperature".into(), None),
        (Method::GET, "https://home.example/proxy/api/states/light.studio".into(), None),
    ]);
}

#[test]
fn a_toggle_rechecks_the_entity_then_calls_its_real_service() {
    let backend = backend([json!({"entity_id":"light.studio","state":"off"}), json!([])]);
    backend.act("http://home.local:8123", &key(), "light.studio", Action::Toggle).unwrap();
    assert_eq!(backend.transport.requests.borrow().as_slice(), [
        (Method::GET, "http://home.local:8123/api/states/light.studio".into(), None),
        (Method::POST, "http://home.local:8123/api/services/light/toggle".into(), Some(json!({"entity_id":"light.studio"}))),
    ]);
}

/// Unavailable and unsupported entities stop after reading state, before posting an action.
#[test]
fn unavailable_and_read_only_entities_never_send_an_action() {
    for value in [json!({"entity_id":"light.studio","state":"unavailable"}), json!({"entity_id":"sensor.temperature","state":"21"}), json!({"entity_id":"lock.front_door","state":"locked"})] {
        let id = value["entity_id"].as_str().unwrap().to_owned();
        let backend = backend([value]);
        assert!(backend.act("https://home.example", &key(), &id, Action::Toggle).is_err());
        assert_eq!(backend.transport.requests.borrow().len(), 1);
    }
}

/// Activation uses each supported domain service and still rejects unavailable entities.
#[test]
fn scene_and_button_actions_use_their_domains() {
    for (domain, service) in [("scene", "turn_on"), ("script", "turn_on"), ("button", "press"), ("input_button", "press")] {
        for state in ["unknown", "2026-11-04T16:00:00Z"] {
            let id = format!("{domain}.evening");
            let backend = backend([json!({"entity_id":id,"state":state}), Value::Null]);
            backend.act("https://home.example", &key(), &id, Action::Activate).unwrap();
            assert!(backend.transport.requests.borrow()[1].1.ends_with(&format!("/api/services/{domain}/{service}")));
        }
        let id = format!("{domain}.evening");
        let backend = backend([json!({"entity_id":id,"state":"unavailable"})]);
        assert!(backend.act("https://home.example", &key(), &id, Action::Activate).is_err());
        assert_eq!(backend.transport.requests.borrow().len(), 1);
    }
}

/// Malformed entity paths and oversized selections are rejected before network access.
#[test]
fn invalid_identifiers_and_large_favourite_lists_do_not_make_requests() {
    let backend = backend([]);
    for id in ["light/../../config", "light.x?token=y", "light.x#fragment", "light.", ".empty", "light.two.dots", "light.%2f"] {
        assert!(!entity_id(id));
        assert!(backend.act("https://home.example", &key(), id, Action::Toggle).is_err());
    }
    assert!(backend.states("https://home.example", &key(), &vec!["light.a".into(); MAX_FAVOURITES + 1]).is_err());
    assert!(backend.transport.requests.borrow().is_empty());
}

/// Only an explicit 404 skips a favourite; authentication and transport failures remain visible.
#[test]
fn missing_favourites_are_skipped_but_other_failures_are_reported() {
    let backend = backend([json!({"entity_id":"light.studio","state":"on"})]);
    let mut missing = crate::web_api::Error::from("Server replied with HTTP 404".to_owned());
    missing.status = Some(404);
    backend.transport.replies.borrow_mut().push_front(Err(missing));
    let states = backend.states("https://home.example", &key(), &["light.removed".into(), "light.studio".into()]).unwrap();
    assert_eq!(states.len(), 1);
    assert_eq!(states[0].id, "light.studio");
    for status in [None, Some(401), Some(403), Some(429), Some(500)] {
        let mut error = crate::web_api::Error::from("fixture failure".to_owned());
        error.status = status;
        backend.transport.replies.borrow_mut().push_back(Err(error));
        assert_eq!(backend.states("https://home.example", &key(), &["light.studio".into()]).unwrap_err(), "fixture failure");
    }
}

#[test]
fn discovery_is_sorted_deduplicated_and_bounded() {
    let mut values = vec![json!({"entity_id":"light.z","state":"off"}); 2];
    values.extend((0..MAX_ENTITIES + 2).map(|i| json!({"entity_id":format!("sensor.n{i}"),"state":"1"})));
    let backend = backend([Value::Array(values)]);
    let states = backend.states("https://home.example", &key(), &[]).unwrap();
    assert_eq!(states.len(), MAX_ENTITIES);
    assert!(states.windows(2).all(|pair| pair[0].name <= pair[1].name));
    assert_eq!(states.iter().filter(|entity| entity.id == "light.z").count(), 1);
    assert_eq!(backend.transport.requests.borrow()[0].1, "https://home.example/api/states");
}

/// A successful HTTP reply still needs the requested entity identity and state.
#[test]
fn a_successful_response_must_describe_the_requested_favourite() {
    for value in [Value::Null, json!({"entity_id":"light.other","state":"on"}), json!({"entity_id":"light.studio"})] {
        let backend = backend([value]);
        assert!(backend.states("https://home.example", &key(), &["light.studio".into()]).is_err());
        assert_eq!(backend.transport.requests.borrow().len(), 1);
    }
}
