//! What every answer of the server has in common, whatever the page.

use http::StatusCode;
use mymcps_web::testing::{TestApp, factories};

#[tokio::test]
async fn never_lets_a_browser_keep_a_copy_of_a_page() {
    let app = TestApp::new().await;
    let admin = factories::create_admin(&app).await;

    for (path, signed_in) in [("/login", false), ("/", true), ("/settings", true)] {
        let request = app.get(path);
        let request = if signed_in {
            request.login_as(&admin)
        } else {
            request
        };
        let response = request.send().await;
        assert_eq!(response.status, StatusCode::OK, "{path}");
        assert_eq!(response.header("cache-control"), Some("no-store"), "{path}");
    }

    // The error pages too.
    let missing = app.get("/no-such-page").login_as(&admin).send().await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
    assert_eq!(missing.header("cache-control"), Some("no-store"));

    // What is not a page keeps the caching it asked for.
    let health = app.get("/health").send().await;
    assert_eq!(health.header("cache-control"), None);
    let stylesheet = app.get("/assets/css/app.css").send().await;
    assert_eq!(stylesheet.status, StatusCode::OK);
    assert_ne!(stylesheet.header("cache-control"), Some("no-store"));
}

#[tokio::test]
async fn does_not_find_a_path_under_a_method_it_has_no_route_for() {
    let app = TestApp::new().await;
    let admin = factories::create_admin(&app).await;

    let page = app.put("/login").csrf().send().await;
    assert_eq!(page.status, StatusCode::NOT_FOUND);
    assert_eq!(page.header("allow"), None);

    let api = app
        .delete("/settings")
        .login_as(&admin)
        .csrf()
        .api()
        .send()
        .await;
    assert_eq!(api.status, StatusCode::NOT_FOUND);
    assert_eq!(
        app.put("/health").api().send().await.status,
        StatusCode::NOT_FOUND
    );
}
