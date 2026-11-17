//! A Device removes itself from the Server when its Agent is uninstalled (`sudo rcli uninstall` on that
//! Device): the Server deletes it and closes the connection. The Agent asks before it deletes its own credentials,
//! so the Device ID and password never outlive the install.

use rcli_testkit::{eventually, StatusCode, TestSystem};

#[tokio::test]
async fn an_agent_removes_its_own_device() {
    let sys = TestSystem::start().await;
    let mut agent = sys.start_agent("old-box").await;
    let id = agent.device_id().to_owned();
    sys.wait_device(&id, |d| d["status"] == "Online").await;

    // A second Device is untouched.
    let other = sys.start_agent("kept-box").await;
    sys.wait_device(other.device_id(), |d| d["status"] == "Online").await;

    // What `rcli uninstall` does before it deletes the local credentials.
    rcli_agent::unregister(&agent.server_access, &agent.creds).await.unwrap();

    // Gone from the Address Book, its detail 404s; the Server closed its connection.
    eventually(|| async {
        let (_, devices) = sys.api.get("/api/v1/address-book").await;
        (!devices.as_array()?.iter().any(|d| d["device_id"] == id)).then_some(())
    })
    .await;
    let (status, body) = sys.api.get(&format!("/api/v1/devices/{id}")).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    sys.wait_device(other.device_id(), |d| d["status"] == "Online").await;

    // The Agent's old credentials can no longer connect: a reconnect is refused and registers nothing.
    agent.stop();
    let err = rcli_agent::unregister(&agent.server_access, &agent.creds).await;
    assert!(err.is_ok(), "removing an already removed Device is not an error: {err:?}");
}
