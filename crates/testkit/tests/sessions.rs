use rcli_testkit::{Clock, StatusCode, TestSystem};
use serde_json::json;

#[tokio::test]
async fn a_user_without_totp_signs_in_with_name_and_password() {
    let sys = TestSystem::start().await;
    sys.create_user("vanna").await;
    let (status, body) =
        sys.api.post_json("/api/v1/users/vanna/credentials", &json!({"password": "correct horse"})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.get("totp_secret").is_none(), "a new password enrols no TOTP: {body}");

    let anon = sys.api.with_token(None);
    let (status, session) = anon.post_json("/api/v1/sessions", &json!({"name": "vanna", "password": "correct horse"})).await;
    assert_eq!(status, StatusCode::CREATED, "{session}");
    let me = sys.api.with_token(session["token"].as_str());
    let (status, user) = me.get("/api/v1/me").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(user["name"], "vanna");
    assert_eq!(me.get("/api/v1/me/totp").await.1, json!({"on": false}));

    // Signing out ends the session.
    assert_eq!(me.delete("/api/v1/session").await.0, StatusCode::NO_CONTENT);
    assert_eq!(me.get("/api/v1/me").await.0, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn turning_totp_on_needs_one_valid_code_and_off_again_drops_it() {
    let sys = TestSystem::start().await;
    let token = sys.create_user("vanna").await;
    let me = sys.api.with_token(Some(&token));
    let anon = &sys.api.with_token(None);
    let sign_in = |body: serde_json::Value| async move { anon.post_json("/api/v1/sessions", &body).await };
    let no_code = json!({"name": "vanna", "password": TestSystem::PASSWORD});

    let (status, enrolled) = me.post_json("/api/v1/me/totp", &json!({})).await;
    assert_eq!(status, StatusCode::OK, "{enrolled}");
    let secret = enrolled["totp_secret"].as_str().unwrap();
    assert!(enrolled["otpauth_url"].as_str().unwrap().starts_with("otpauth://totp/Remote%20CLI:vanna?"), "{enrolled}");
    let code = sys.totp(secret);
    let wrong_code = format!("{:06}", (code.parse::<u32>().unwrap() + 1) % 1_000_000);

    // A wrong code leaves TOTP off.
    let (status, body) = me.put_json("/api/v1/me/totp", &json!({"secret": secret, "code": wrong_code})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"]["code"], "invalid_code");
    assert_eq!(me.get("/api/v1/me/totp").await.1, json!({"on": false}));
    assert_eq!(sign_in(no_code.clone()).await.0, StatusCode::CREATED);

    let (status, body) = me.put_json("/api/v1/me/totp", &json!({"secret": secret, "code": code})).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    assert_eq!(me.get("/api/v1/me/totp").await.1, json!({"on": true}));
    sys.advance(30); // the code that confirmed is spent

    // On: no code, or a wrong one, is refused; the current code signs in.
    let (status, body) = sign_in(no_code.clone()).await;
    assert_eq!((status, body["error"]["code"].clone()), (StatusCode::UNAUTHORIZED, json!("totp_required")), "{body}");
    let mut with_code = no_code.clone();
    with_code["totp"] = json!(wrong_code);
    let (status, body) = sign_in(with_code.clone()).await;
    assert_eq!((status, body["error"]["code"].clone()), (StatusCode::UNAUTHORIZED, json!("invalid_credentials")), "{body}");
    with_code["totp"] = json!(sys.totp(secret));
    assert_eq!(sign_in(with_code.clone()).await.0, StatusCode::CREATED);

    // A session alone cannot replace the secret or turn TOTP off: that needs a current code.
    let other = me.post_json("/api/v1/me/totp", &json!({})).await.1["totp_secret"].as_str().unwrap().to_owned();
    let (status, body) = me.put_json("/api/v1/me/totp", &json!({"secret": other, "code": sys.totp(&other)})).await;
    assert_eq!((status, body["error"]["code"].clone()), (StatusCode::CONFLICT, json!("totp_on")), "{body}");
    let (status, body) = me.delete_json("/api/v1/me/totp", &json!({"code": wrong_code})).await;
    assert_eq!((status, body["error"]["code"].clone()), (StatusCode::BAD_REQUEST, json!("invalid_code")), "{body}");
    assert_eq!(me.get("/api/v1/me/totp").await.1, json!({"on": true}));
    sys.advance(30);

    // Off again: name and password are enough.
    assert_eq!(me.delete_json("/api/v1/me/totp", &json!({"code": sys.totp(secret)})).await.0, StatusCode::NO_CONTENT);
    assert_eq!(sign_in(no_code).await.0, StatusCode::CREATED);
}

#[tokio::test]
async fn sign_in_is_refused_without_the_right_password_and_a_fresh_totp() {
    let sys = TestSystem::start().await;
    sys.create_dashboard_user("vanna").await;
    let secret = sys.turn_on_totp(&sys.sign_in("vanna").await).await;
    sys.advance(30);
    let code = sys.totp(&secret);
    let anon = &sys.api.with_token(None);
    let sign_in = |name: &str, password: &str, totp: &str| {
        let body = json!({"name": name, "password": password, "totp": totp});
        async move { anon.post_json("/api/v1/sessions", &body).await }
    };

    for (name, password) in [("vanna", "wrong"), ("nobody", TestSystem::PASSWORD)] {
        let (status, body) = sign_in(name, password, &code).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{name}/{password}: {body}");
        assert_eq!(body["error"]["code"], "invalid_credentials");
    }
    assert_eq!(sign_in("vanna", TestSystem::PASSWORD, &code).await.0, StatusCode::CREATED);
    // The same code cannot be replayed.
    assert_eq!(sign_in("vanna", TestSystem::PASSWORD, &code).await.0, StatusCode::UNAUTHORIZED);
    // The next 30 s step brings a fresh code.
    sys.advance(30);
    assert_eq!(sign_in("vanna", TestSystem::PASSWORD, &sys.totp(&secret)).await.0, StatusCode::CREATED);
}

#[tokio::test]
async fn sessions_expire_and_disabled_users_cannot_sign_in() {
    let sys = TestSystem::start().await;
    sys.create_user("vanna").await;
    sys.api.post_json("/api/v1/users/vanna/credentials", &json!({"password": "pw-1234567"})).await;
    let anon = sys.api.with_token(None);
    let (_, session) = anon.post_json("/api/v1/sessions", &json!({"name": "vanna", "password": "pw-1234567"})).await;
    let me = sys.api.with_token(session["token"].as_str());
    assert_eq!(me.get("/api/v1/address-book").await.0, StatusCode::OK);

    let mut sys = sys;
    sys.advance(12 * 3600);
    assert_eq!(me.get("/api/v1/address-book").await.0, StatusCode::UNAUTHORIZED);
    sys.sign_in_admin().await; // the harness admin's session expired too

    sys.api.post("/api/v1/users/vanna/disable").await;
    let (status, _) = anon.post_json("/api/v1/sessions", &json!({"name": "vanna", "password": "pw-1234567"})).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn passwords_must_be_8_chars() {
    let sys = TestSystem::start().await;
    sys.create_user("op").await;
    let (status, body) = sys.api.post_json("/api/v1/users/op/credentials", &json!({"password": "short"})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let (status, _) = sys.api.post_json("/api/v1/users/ghost/credentials", &json!({"password": "longenough"})).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn admin_sets_credentials_with_rcli() {
    let sys = TestSystem::start().await;
    sys.create_user("vanna").await;
    sys.rcli(&["user", "set-password", "vanna", "--password", "pw-1234567"]).await.unwrap();
    let (status, _) = sys
        .api
        .with_token(None)
        .post_json("/api/v1/sessions", &json!({"name": "vanna", "password": "pw-1234567"}))
        .await;
    assert_eq!(status, StatusCode::CREATED);
}

#[tokio::test]
async fn five_failed_sign_ins_lock_the_user_out_for_15_minutes() {
    let sys = TestSystem::start().await;
    sys.create_user("vanna").await;
    sys.api.post_json("/api/v1/users/vanna/credentials", &json!({"password": "pw-1234567"})).await;
    let anon = &sys.api.with_token(None);
    let sign_in = |password: &str| {
        let body = json!({"name": "vanna", "password": password});
        async move { anon.post_json("/api/v1/sessions", &body).await }
    };

    // A success resets the count.
    for _ in 0..4 {
        assert_eq!(sign_in("wrong").await.0, StatusCode::UNAUTHORIZED);
    }
    assert_eq!(sign_in("pw-1234567").await.0, StatusCode::CREATED);
    for _ in 0..5 {
        assert_eq!(sign_in("wrong").await.0, StatusCode::UNAUTHORIZED);
    }

    // Locked: even the right password is refused, with a typed error saying until when.
    let (status, body) = sign_in("pw-1234567").await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{body}");
    assert_eq!(body["error"]["code"], "too_many_attempts");
    let until: chrono::DateTime<chrono::Utc> = body["error"]["locked_until"].as_str().unwrap().parse().unwrap();
    assert_eq!(until, sys.clock.now() + chrono::Duration::minutes(15));

    sys.advance(14 * 60);
    assert_eq!(sign_in("pw-1234567").await.0, StatusCode::TOO_MANY_REQUESTS);
    sys.advance(60);
    assert_eq!(sign_in("pw-1234567").await.0, StatusCode::CREATED);
}
