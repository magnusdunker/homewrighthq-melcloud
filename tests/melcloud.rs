//! Mapping tests on responses shaped like MELCloud's, and the plugin against
//! a fake MELCloud with HomewrightHQ played over a pipe.

mod host;

use homewright_melcloud::model::{self, atw_flags, ata_flags};
use homewright_melcloud::MelCloud;
use serde_json::{json, Map, Value};
use wiremock::matchers::{body_partial_json, header, method, path, query_param};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

fn fixture(name: &str) -> Value {
    let p = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

fn obj(v: Value) -> Map<String, Value> {
    v.as_object().cloned().unwrap()
}

#[test]
fn devices_from_all_levels_of_a_building() {
    let d = model::parse_devices(&fixture("list_devices.json"));
    assert_eq!(d.len(), 3);
    let ata = d.iter().find(|d| d.id == 1001).unwrap();
    assert_eq!(ata.name, "Stue");
    assert_eq!(ata.building_id, 501);
    assert_eq!(ata.kind(), "Ata");
    assert_eq!(ata.model.as_deref(), Some("MSZ-LN25VG2W"));
    assert_eq!(ata.serial.as_deref(), Some("2201234"));
    let atw = d.iter().find(|d| d.id == 2002).unwrap();
    assert_eq!(atw.kind(), "Atw");
    assert!(model::has_zone2(&atw.conf));
    assert_eq!(model::zone_name(&atw.conf, 1), "Gulvvarme");
    assert_eq!(model::zone_name(&atw.conf, 2), "Zone 2");
    // an energy recovery ventilator is listed but not supported
    assert_eq!(d.iter().find(|d| d.id == 3003).unwrap().kind(), "");
}

fn ata() -> (Map<String, Value>, Map<String, Value>) {
    let d = model::parse_devices(&fixture("list_devices.json"));
    let conf = d.into_iter().find(|d| d.id == 1001).unwrap().conf;
    (obj(fixture("ata_get.json")), conf)
}

#[test]
fn air_to_air_state() {
    let (s, c) = ata();
    let (state, a) = model::ata_climate(&s, &c);
    assert_eq!(state, "heat");
    assert_eq!(a["hvac_modes"], json!(["off", "heat", "cool", "dry", "fan_only", "heat_cool"]));
    assert_eq!(a["temperature"], 22.0);
    assert_eq!(a["current_temperature"], 21.5);
    assert_eq!(a["min_temp"], 10.0);
    assert_eq!(a["fan_mode"], "auto");
    assert_eq!(a["fan_modes"], json!(["auto", "1", "2", "3", "4", "5"]));
    assert_eq!(model::vane_vertical(&s), Some("swing"));
    assert_eq!(model::vane_horizontal(&s), Some("3"));
    assert_eq!(model::energy_kwh(&c), Some(1234.567));

    let mut off = s.clone();
    off.insert("Power".into(), json!(false));
    assert_eq!(model::ata_climate(&off, &c).0, "off");
}

#[test]
fn air_to_air_commands() {
    let (s, c) = ata();
    let d = |v: Value| obj(v);
    assert_eq!(model::ata_change("set_hvac_mode", &d(json!({ "hvac_mode": "off" })), &s, &c).unwrap(), (d(json!({ "Power": false })), ata_flags::POWER));
    assert_eq!(
        model::ata_change("set_hvac_mode", &d(json!({ "hvac_mode": "cool" })), &s, &c).unwrap(),
        (d(json!({ "Power": true, "OperationMode": 3 })), ata_flags::POWER | ata_flags::OPERATION_MODE)
    );
    // rounded to the step and kept inside the heating range
    assert_eq!(model::ata_change("set_temperature", &d(json!({ "temperature": 22.3 })), &s, &c).unwrap(), (d(json!({ "SetTemperature": 22.5 })), ata_flags::TEMPERATURE));
    assert_eq!(model::ata_change("set_temperature", &d(json!({ "temperature": 5 })), &s, &c).unwrap().0["SetTemperature"], 10.0);
    // in cooling the floor is 16
    let (m, f) = model::ata_change("set_temperature", &d(json!({ "temperature": 5, "hvac_mode": "cool" })), &s, &c).unwrap();
    assert_eq!(m["SetTemperature"], 16.0);
    assert_eq!(f, ata_flags::POWER | ata_flags::OPERATION_MODE | ata_flags::TEMPERATURE);
    assert_eq!(model::ata_change("set_fan_mode", &d(json!({ "fan_mode": "3" })), &s, &c).unwrap(), (d(json!({ "SetFanSpeed": 3 })), ata_flags::FAN_SPEED));
    assert!(model::ata_change("set_fan_mode", &d(json!({ "fan_mode": "6" })), &s, &c).is_err());
    assert_eq!(model::ata_change("set_swing_mode", &Map::new(), &s, &c).unwrap_err(), "");
    assert_eq!(model::set_vane_horizontal("split").unwrap(), (d(json!({ "VaneHorizontal": 8 })), ata_flags::VANE_HORIZONTAL));
    assert!(model::set_vane_vertical("sideways").is_none());

    let cmd = model::command(&s, &d(json!({ "SetFanSpeed": 3 })), ata_flags::FAN_SPEED);
    assert_eq!(cmd["SetFanSpeed"], 3);
    assert_eq!(cmd["EffectiveFlags"], 8);
    assert_eq!(cmd["HasPendingCommand"], true);
    assert_eq!(cmd["DeviceID"], 1001);
}

fn atw() -> (Map<String, Value>, Map<String, Value>) {
    let d = model::parse_devices(&fixture("list_devices.json"));
    let conf = d.into_iter().find(|d| d.id == 2002).unwrap().conf;
    (obj(fixture("atw_get.json")), conf)
}

#[test]
fn air_to_water_state() {
    let (s, c) = atw();
    let (state, a) = model::water_heater(&s, &c);
    assert_eq!(state, "auto");
    assert_eq!(a["current_temperature"], 47.5);
    assert_eq!(a["temperature"], 50.0);
    assert_eq!(a["max_temp"], 60.0);

    let (state, a) = model::zone_climate(&s, &c, 1);
    assert_eq!(state, "heat");
    assert_eq!(a["hvac_action"], "heating");
    assert_eq!(a["preset_mode"], "room");
    assert_eq!(a["temperature"], 21.0);
    assert_eq!(a["hvac_modes"], json!(["off", "heat"]));
    let (_, a) = model::zone_climate(&s, &c, 2);
    assert_eq!(a["hvac_action"], "idle");
    assert_eq!(a["preset_mode"], "curve");

    assert_eq!(model::atw_status(&s), Some("heating"));
    assert_eq!(model::flow_target(&s, 1), Some(35.0));
    // flow and return come from the device list
    assert_eq!(model::temperature_sensor(&s, &c, "FlowTemperature"), Some(34.5));
    assert_eq!(model::temperature_sensor(&s, &c, "OutdoorTemperature"), Some(4.5));
}

#[test]
fn air_to_water_commands() {
    let (s, c) = atw();
    let d = |v: Value| obj(v);
    assert_eq!(
        model::water_change("set_temperature", &d(json!({ "temperature": 75 })), &c).unwrap(),
        (d(json!({ "SetTankWaterTemperature": 60.0 })), atw_flags::TANK_TEMPERATURE)
    );
    assert_eq!(
        model::water_change("set_operation_mode", &d(json!({ "operation_mode": "force_hot_water" })), &c).unwrap(),
        (d(json!({ "ForcedHotWaterMode": true })), atw_flags::FORCED_HOT_WATER)
    );
    assert_eq!(
        model::zone_change("set_temperature", &d(json!({ "temperature": 21.7 })), &s, 2).unwrap(),
        (d(json!({ "SetTemperatureZone2": 21.5 })), atw_flags::ZONE2_TEMPERATURE)
    );
    assert_eq!(
        model::zone_change("set_preset_mode", &d(json!({ "preset_mode": "flow" })), &s, 1).unwrap(),
        (d(json!({ "OperationModeZone1": 1 })), atw_flags::ZONE1_MODE)
    );
    // cooling keeps the room or flow choice
    assert_eq!(
        model::zone_change("set_hvac_mode", &d(json!({ "hvac_mode": "cool" })), &s, 1).unwrap(),
        (d(json!({ "Power": true, "OperationModeZone1": 3 })), atw_flags::POWER | atw_flags::ZONE1_MODE)
    );
    assert_eq!(model::zone_change("set_hvac_mode", &d(json!({ "hvac_mode": "off" })), &s, 1).unwrap(), (d(json!({ "Power": false })), atw_flags::POWER));
    assert_eq!(model::set_flow_target(1, 70.0), (d(json!({ "SetHeatFlowTemperatureZone1": 60.0 })), atw_flags::FLOW_TEMPERATURE));
}

async fn cloud(server: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/Login/ClientLogin"))
        .and(body_partial_json(json!({ "Email": "a@b.dk", "Password": "secret" })))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture("login.json")))
        .mount(server)
        .await;
    Mock::given(method("POST"))
        .and(path("/Login/ClientLogin"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "ErrorId": 1, "ErrorMessage": null, "LoginData": null })))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/User/ListDevices"))
        .and(header("X-MitsContextKey", "CTX-1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture("list_devices.json")))
        .mount(server)
        .await;
    for (id, file) in [("1001", "ata_get.json"), ("2002", "atw_get.json")] {
        Mock::given(method("GET"))
            .and(path("/Device/Get"))
            .and(query_param("id", id))
            .and(query_param("buildingID", "501"))
            .and(header("X-MitsContextKey", "CTX-1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(fixture(file)))
            .mount(server)
            .await;
    }
    // MELCloud answers a command with the state it will send
    for kind in ["Ata", "Atw"] {
        Mock::given(method("POST"))
            .and(path(format!("/Device/Set{kind}")))
            .respond_with(|req: &Request| ResponseTemplate::new(200).set_body_json(serde_json::from_slice::<Value>(&req.body).unwrap()))
            .mount(server)
            .await;
    }
}

fn flow(data: Value) -> Value {
    json!({ "flow_id": "f1", "source": "user", "data": data })
}

#[tokio::test(flavor = "multi_thread")]
async fn flow_setup_and_commands() {
    let server = MockServer::start().await;
    cloud(&server).await;
    let h = host::FakeHost::start(MelCloud::default());
    let base = json!({ "base_url": server.uri() });

    let r = h.call("config_flow", json!({ "flow": flow(base.clone()), "step": "user", "input": null })).await.unwrap();
    assert_eq!(r["result"]["type"], "form");
    let keys: Vec<&String> = r["result"]["schema"]["properties"].as_object().unwrap().keys().collect();
    assert_eq!(keys, ["email", "password"]);

    let r = h.call("config_flow", json!({ "flow": flow(base.clone()), "step": "user", "input": { "email": "a@b.dk", "password": "wrong" } })).await.unwrap();
    assert_eq!(r["result"]["errors"]["base"], "invalid_auth");

    let r = h.call("config_flow", json!({ "flow": flow(base.clone()), "step": "user", "input": { "email": " a@b.dk ", "password": "secret" } })).await.unwrap();
    assert_eq!(r["result"]["type"], "create_entry");
    assert_eq!(r["result"]["title"], "MELCloud (a@b.dk)");
    assert_eq!(r["unique_id"], "a@b.dk");
    let data = r["result"]["data"].clone();

    h.call("setup_entry", json!({ "entry": { "entry_id": "e1", "domain": "melcloud", "data": data } })).await.unwrap();
    assert_eq!(h.rec.lock().unwrap().devices.len(), 2);

    let climate = h.entity("1001");
    let (state, attrs) = h.state(&climate).unwrap();
    assert_eq!(state, "heat");
    assert_eq!(attrs["temperature"], 22.0);
    assert_eq!(h.state(&h.entity("1001-room")).unwrap().0, "21.5");
    assert_eq!(h.state(&h.entity("1001-vane_vertical")).unwrap().0, "swing");
    assert_eq!(h.state(&h.entity("1001-energy")).unwrap().0, "1234.567");
    assert_eq!(h.state(&h.entity("2002-tank")).unwrap().0, "auto");
    assert_eq!(h.state(&h.entity("2002-zone1")).unwrap().0, "heat");
    assert_eq!(h.state(&h.entity("2002-zone2_flow_target")).unwrap().0, "30");
    assert_eq!(h.state(&h.entity("2002-flow")).unwrap().0, "34.5");
    assert_eq!(h.state(&h.entity("2002-status")).unwrap().0, "heating");
    let zone1 = h.entity("2002-zone1");
    let (spec, handler) = h.rec.lock().unwrap().entities[&zone1].clone();
    assert_eq!(spec["name"], "Gulvvarme");
    assert!(handler);

    let call = |entity: String, service: &str, data: Value| json!({ "entry_id": "e1", "entity_id": entity, "service": service, "data": data });
    h.call("entity/call_service", call(climate.clone(), "set_hvac_mode", json!({ "hvac_mode": "cool" }))).await.unwrap();
    assert_eq!(h.state(&climate).unwrap().0, "cool");
    h.call("entity/call_service", call(h.entity("2002-tank"), "set_temperature", json!({ "temperature": 55 }))).await.unwrap();
    assert_eq!(h.state(&h.entity("2002-tank")).unwrap().1["temperature"], 55.0);
    h.call("entity/call_service", call(h.entity("2002-zone1_flow_target"), "set_value", json!({ "value": 38 }))).await.unwrap();
    assert_eq!(h.state(&h.entity("2002-zone1_flow_target")).unwrap().0, "38");

    let err = h.call("entity/call_service", call(climate.clone(), "set_fan_mode", json!({ "fan_mode": "turbo" }))).await.unwrap_err();
    assert_eq!(err["data"]["kind"], "invalid_argument");

    let sent: Vec<Value> = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| r.url.path().starts_with("/Device/Set"))
        .map(|r| serde_json::from_slice(&r.body).unwrap())
        .collect();
    assert_eq!(sent.len(), 3);
    assert_eq!(sent[0]["OperationMode"], 3);
    assert_eq!(sent[0]["EffectiveFlags"], ata_flags::POWER | ata_flags::OPERATION_MODE);
    assert_eq!(sent[0]["DeviceID"], 1001);
    assert_eq!(sent[1]["SetTankWaterTemperature"], 55.0);
    assert_eq!(sent[1]["EffectiveFlags"], atw_flags::TANK_TEMPERATURE);
    assert_eq!(sent[2]["SetHeatFlowTemperatureZone1"], 38.0);

    let diag = h.call("diagnostics", json!({ "entry": { "entry_id": "e1", "data": { "email": "a@b.dk", "password": "secret" } } })).await.unwrap();
    assert_eq!(diag["entry"]["password"], "**REDACTED**");

    let opts = h.call("options_flow", json!({ "flow": { "flow_id": "o1", "source": "options", "entry": { "entry_id": "e1", "data": {} } }, "step": "init", "input": { "scan_interval": 1 } })).await.unwrap();
    assert_eq!(opts["result"]["data"]["scan_interval"], 3);
}

#[tokio::test(flavor = "multi_thread")]
async fn expired_key_logs_in_again_and_rate_limit_is_not_auth() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/Login/ClientLogin"))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture("login.json")))
        .mount(&server)
        .await;
    // the first answer is 401, the next one works
    Mock::given(method("GET"))
        .and(path("/User/ListDevices"))
        .respond_with(ResponseTemplate::new(401))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/User/ListDevices"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&server)
        .await;
    let c = homewright_melcloud::api::Client::new(&server.uri(), "a@b.dk", "x");
    assert_eq!(c.list_devices().await.unwrap(), json!([]));
    let logins = server.received_requests().await.unwrap().iter().filter(|r| r.url.path() == "/Login/ClientLogin").count();
    assert_eq!(logins, 2);

    let limited = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/Login/ClientLogin"))
        .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "3600"))
        .mount(&limited)
        .await;
    let c = homewright_melcloud::api::Client::new(&limited.uri(), "a@b.dk", "x");
    assert_eq!(c.login().await.unwrap_err(), homewright_melcloud::api::ApiError::RateLimited(Some(std::time::Duration::from_secs(3600))));
}
