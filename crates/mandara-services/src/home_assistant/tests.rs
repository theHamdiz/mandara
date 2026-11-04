use std::cell::RefCell;
use std::collections::VecDeque;
use serde_json::Value;

use super::*;

#[derive(Default)]
struct Fixture {
    replies: RefCell<VecDeque<Value>>,
    requests: RefCell<Vec<(Method, String, Option<Value>)>>,
}

impl Transport for Fixture {
    fn json(&self, method: Method, url: &str, body: Option<&Value>, auth: Option<Authorization<'_>>) -> Result<Value, String> {
        assert!(matches!(auth, Some(Authorization::Bearer(_))));
        self.requests.borrow_mut().push((method, url.into(), body.cloned()));
        self.replies.borrow_mut().pop_front().ok_or_else(|| "unexpected request".into())
    }
}

fn backend(replies: impl IntoIterator<Item = Value>) -> Http<Fixture> {
    Http { transport: Fixture { replies: RefCell::new(replies.into_iter().collect()), ..Default::default() } }
}

fn key() -> Secret { Secret::new("private-fixture-token".into()).unwrap() }

#[test]
fn favourites_keep_their_order_units_and_server_names() {
    let backend = backend([json!([
        {"entity_id":"light.studio","state":"off","attributes":{"friendly_name":"Studio light"}},
        {"entity_id":"sensor.temperature","state":"21.8","attributes":{"friendly_name":"Temperature","unit_of_measurement":"°C"}},
        {"entity_id":"switch.other","state":"on"},
        {"entity_id":"light.studio","state":"on"}
    ])]);
    let favourites = ["sensor.temperature".into(), "light.studio".into()];
    let states = backend.states("http://home.local:8123/proxy/", &key(), &favourites).unwrap();
    assert_eq!(states.iter().map(|entity| entity.id.as_str()).collect::<Vec<_>>(), ["sensor.temperature", "light.studio"]);
    assert_eq!(states[0].unit.as_deref(), Some("°C"));
    assert_eq!(states[1].name, "Studio light");
    assert_eq!(states[1].state, "off");
    assert_eq!(backend.transport.requests.borrow()[0].1, "http://home.local:8123/proxy/api/states");
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

#[test]
fn unavailable_and_read_only_entities_never_send_an_action() {
    for value in [json!({"entity_id":"light.studio","state":"unavailable"}), json!({"entity_id":"sensor.temperature","state":"21"}), json!({"entity_id":"lock.front_door","state":"locked"})] {
        let id = value["entity_id"].as_str().unwrap().to_owned();
        let backend = backend([value]);
        assert!(backend.act("https://home.example", &key(), &id, Action::Toggle).is_err());
        assert_eq!(backend.transport.requests.borrow().len(), 1);
    }
}

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
