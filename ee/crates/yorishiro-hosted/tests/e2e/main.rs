//! Browser tests for the flows a person actually performs.
//!
//! `curl` proves the API, not the product: the SPA serves `index.html` for any unmatched path, so a wrong URL answers 200 from the shell and a login can succeed over HTTP while failing in the browser.
//! These drive a real Chrome and read what the page says.
//!
//! **Every test here is `#[ignore]`.** They need a running licensed server with data in it, which `cargo test` has no way to provide, and a suite that silently passes when its dependencies are missing is worse than one that is not run at all.
//! Run them deliberately:
//!
//! ```sh
//! chromedriver --port=9515 &
//! YORISHIRO_E2E_URL=http://localhost:18081 \
//!   YORISHIRO_E2E_EMAIL=... YORISHIRO_E2E_PASSWORD=... \
//!   cargo test -p yorishiro-hosted --test e2e -- --ignored --test-threads=1
//! ```
//!
//! `--test-threads=1` because they share one browser session's worth of server state, not because any single test is order-dependent.

use std::time::Duration;

use thirtyfour::components::SelectElement;
use thirtyfour::prelude::*;

/// Where the suite expects a server, and who to sign in as.
///
/// Read from the environment rather than defaulted, so a missing value fails the test with the name of what is missing instead of quietly driving a browser at the wrong deployment.
struct Env {
    base_url: String,
    email: String,
    password: String,
    webdriver: String,
}

impl Env {
    fn from_env() -> Self {
        fn required(key: &str) -> String {
            std::env::var(key)
                .unwrap_or_else(|_| panic!("{key} must be set to run the browser suite"))
        }
        Self {
            base_url: required("YORISHIRO_E2E_URL"),
            email: required("YORISHIRO_E2E_EMAIL"),
            password: required("YORISHIRO_E2E_PASSWORD"),
            // The one with a default: chromedriver's port is a local detail, not a deployment fact.
            webdriver: std::env::var("YORISHIRO_E2E_WEBDRIVER")
                .unwrap_or_else(|_| "http://localhost:9515".into()),
        }
    }
}

async fn browser(env: &Env) -> WebDriver {
    let mut caps = DesiredCapabilities::chrome();
    caps.add_arg("--headless=new").unwrap();
    // Runners have no /dev/shm worth speaking of, and Chrome crashes rather than degrading.
    caps.add_arg("--no-sandbox").unwrap();
    caps.add_arg("--disable-dev-shm-usage").unwrap();
    caps.add_arg("--window-size=1500,1000").unwrap();
    WebDriver::new(&env.webdriver, caps)
        .await
        .expect("could not reach chromedriver; is it running on YORISHIRO_E2E_WEBDRIVER?")
}

/// The session the whole suite shares, minted once.
///
/// Sign-in is rate limited, which is correct behaviour and not something to raise the limit for: four tests signing in four times tripped it and the suite failed every other run.
/// So one test performs the real sign-in and the rest reuse what it produced, which also keeps the credential path exercised exactly once rather than not at all.
static SESSION: tokio::sync::OnceCell<String> = tokio::sync::OnceCell::const_new();

/// Signs in and leaves the browser on the dashboard.
///
/// Asserts on the alert as well as the URL: a wrong password answers 200 and re-renders the login page, so "did not navigate" and "navigated somewhere unexpected" are different failures and only one of them is about credentials.
/// Returns `false` when the auth rate limit refused the attempt, rather than treating that as a credential failure.
async fn sign_in(driver: &WebDriver, env: &Env) -> WebDriverResult<bool> {
    driver.goto(format!("{}/login", env.base_url)).await?;

    driver
        .query(By::Css("input[type=\"email\"]"))
        .wait(Duration::from_secs(15), Duration::from_millis(250))
        .first()
        .await?
        .send_keys(&env.email)
        .await?;
    driver
        .find(By::Css("input[type=\"password\"]"))
        .await?
        .send_keys(&env.password)
        .await?;
    driver
        .find(By::Css("button[type=\"submit\"]"))
        .await?
        .click()
        .await?;

    // The alert is what distinguishes a rejected sign-in from a slow one.
    for _ in 0..40 {
        if driver.current_url().await?.path().starts_with("/dashboard") {
            return Ok(true);
        }
        if let Ok(alert) = driver.find(By::Css("[role=\"alert\"]")).await
            && let Ok(text) = alert.text().await
            && !text.trim().is_empty()
        {
            // Not a credential problem: the auth limit is 10 requests per minute per IP across
            // `/auth/login`, `/auth/signup` **and** `/setup`, and the SPA checks `/setup` on load,
            // so a run's budget is spent by page loads as much as by sign-ins. Two runs inside one
            // window exhaust it however few times the suite authenticates. That is the product
            // working, so the caller waits rather than the deployment weakening a limit for a test.
            if text.contains("Too Many Requests") {
                return Ok(false);
            }
            panic!("sign-in was refused: {text}");
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    panic!(
        "sign-in neither succeeded nor reported an error; still at {}",
        driver.current_url().await?
    );
}

/// Signs in, waiting out the auth rate limit if it is in effect.
///
/// One window is 60 seconds by default, so this can wait that long once rather than failing a suite whose only problem is that it was run twice in a minute.
async fn sign_in_patiently(driver: &WebDriver, env: &Env) -> WebDriverResult<()> {
    if sign_in(driver, env).await? {
        return Ok(());
    }
    eprintln!("auth rate limit in effect; waiting out the window before retrying");
    tokio::time::sleep(Duration::from_secs(62)).await;
    assert!(
        sign_in(driver, env).await?,
        "still rate limited after a full window, which is longer than the limit's own"
    );
    Ok(())
}

/// Puts the browser in a signed-in state without spending a sign-in against the rate limit.
///
/// The first caller signs in for real and keeps what the SPA stored; every later one writes that back into `sessionStorage` before loading the app.
/// The value is the SPA's own session blob, so this is the same state a real sign-in leaves behind rather than a fabricated one.
async fn signed_in(driver: &WebDriver, env: &Env) -> WebDriverResult<()> {
    let session = SESSION
        .get_or_try_init(|| async {
            sign_in_patiently(driver, env).await?;
            let raw: serde_json::Value = driver
                .execute(
                    "return sessionStorage.getItem('yorishiro_session');",
                    vec![],
                )
                .await?
                .convert()
                .expect("sign-in stored no session");
            WebDriverResult::Ok(raw.as_str().expect("session was not a string").to_string())
        })
        .await?;

    // `sessionStorage` is per origin, so the page has to be loaded before it can be written.
    driver.goto(format!("{}/login", env.base_url)).await?;
    driver
        .execute(
            "sessionStorage.setItem('yorishiro_session', arguments[0]);",
            vec![serde_json::Value::String(session.clone())],
        )
        .await?;
    Ok(())
}

/// The workspace id the session is scoped to, read from where the SPA keeps it.
async fn workspace_id(driver: &WebDriver) -> WebDriverResult<String> {
    let raw: serde_json::Value = driver
        .execute(
            "return JSON.parse(sessionStorage.getItem('yorishiro_session') ?? '{}').workspaceId ?? null;",
            vec![],
        )
        .await?
        .convert()
        .expect("session had no workspaceId, so sign-in did not complete");
    Ok(raw
        .as_str()
        .expect("workspaceId was not a string")
        .to_string())
}

/// Column headers currently rendered.
async fn headers(driver: &WebDriver) -> WebDriverResult<Vec<String>> {
    let mut out = Vec::new();
    for th in driver.find_all(By::Css("thead th")).await? {
        out.push(th.text().await?.trim().to_string());
    }
    Ok(out)
}

/// Selects an entity type and waits for the table to be rebuilt for it.
async fn select_entity_type(driver: &WebDriver, entity_type: &str) -> WebDriverResult<()> {
    let select = driver
        .query(By::Css("select[aria-label=\"Filter by entity type\"]"))
        .wait(Duration::from_secs(15), Duration::from_millis(250))
        .first()
        .await?;
    SelectElement::new(&select)
        .await?
        .select_by_value(entity_type)
        .await?;

    // Waits for the Columns button, which renders only once a type is selected, rather than for
    // the header count: a workspace whose saved set is exactly the three built-ins would satisfy
    // a count check while still showing the "All types" table, and the assertion after it would
    // read the wrong page. That is not hypothetical; it is what made this suite fail one run in
    // four before the wait keyed off something that means what it is being asked.
    //
    // A timeout is an error rather than a silent return, so "the table never rebuilt" cannot
    // reach the caller disguised as "the table rebuilt into something unexpected".
    driver
        .query(By::XPath("//button[contains(., 'Columns')]"))
        .wait(Duration::from_secs(15), Duration::from_millis(250))
        .first()
        .await?;
    Ok(())
}

/// Signing in has to work in a browser, not only over HTTP.
///
/// This is the one that caught a password that had never worked: the API accepted the request and the page answered `Session expired. Please sign in again.`, which names neither the account nor the reason.
#[tokio::test]
#[ignore = "needs a running server and chromedriver"]
async fn a_person_can_sign_in_and_reach_the_dashboard() {
    let env = Env::from_env();
    let driver = browser(&env).await;

    let result = async {
        sign_in_patiently(&driver, &env).await?;
        let url = driver.current_url().await?;
        assert!(
            url.path().starts_with("/dashboard"),
            "expected the dashboard, got {url}"
        );
        WebDriverResult::Ok(())
    }
    .await;

    driver.quit().await.ok();
    result.unwrap();
}

/// The table's columns come from the schema, which is the whole point of the feature.
///
/// Asserts that at least one header is not one of the three built-ins: pinning the exact field names would make this fail whenever the workspace's schema changes, which is not what it is guarding.
#[tokio::test]
#[ignore = "needs a running server and chromedriver"]
async fn the_table_shows_columns_the_schema_defines() {
    let env = Env::from_env();
    let driver = browser(&env).await;

    let result = async {
        signed_in(&driver, &env).await?;
        let ws = workspace_id(&driver).await?;
        driver
            .goto(format!("{}/ws/{ws}/entities", env.base_url))
            .await?;
        select_entity_type(&driver, "task").await?;

        let rendered = headers(&driver).await?;
        let built_in = ["Name", "Type", "Created"];
        let from_schema: Vec<_> = rendered
            .iter()
            .filter(|h| !built_in.contains(&h.as_str()))
            .collect();
        assert!(
            !from_schema.is_empty(),
            "no schema-derived column was rendered; headers were {rendered:?}"
        );
        WebDriverResult::Ok(())
    }
    .await;

    driver.quit().await.ok();
    result.unwrap();
}

/// A saved choice must survive a reload, since that is what distinguishes storage from React state.
///
/// Ends by resetting, so the suite can run twice without the second run starting from the first one's leftovers.
#[tokio::test]
#[ignore = "needs a running server and chromedriver"]
async fn a_chosen_column_set_survives_a_reload() {
    let env = Env::from_env();
    let driver = browser(&env).await;

    let result = async {
        signed_in(&driver, &env).await?;
        let ws = workspace_id(&driver).await?;
        driver
            .goto(format!("{}/ws/{ws}/entities", env.base_url))
            .await?;
        select_entity_type(&driver, "task").await?;

        let before = headers(&driver).await?;

        // Open the picker and turn on whichever column is currently off.
        driver
            .query(By::XPath("//button[contains(., 'Columns')]"))
            .wait(Duration::from_secs(10), Duration::from_millis(250))
            .first()
            .await?
            .click()
            .await?;
        let boxes = driver
            .query(By::Css("input[type=\"checkbox\"]"))
            .wait(Duration::from_secs(10), Duration::from_millis(250))
            .all_from_selector()
            .await?;
        let mut toggled = false;
        for b in &boxes {
            if !b.is_selected().await? {
                b.click().await?;
                toggled = true;
                break;
            }
        }
        assert!(toggled, "every column was already on, so nothing was saved");

        driver
            .find(By::XPath("//button[normalize-space()='Save']"))
            .await?
            .click()
            .await?;
        tokio::time::sleep(Duration::from_secs(2)).await;
        let after_save = headers(&driver).await?;
        assert!(
            after_save.len() > before.len(),
            "saving did not add a column: {before:?} -> {after_save:?}"
        );

        // The actual assertion: a full reload, so anything held only in memory is gone.
        driver
            .goto(format!("{}/ws/{ws}/entities", env.base_url))
            .await?;
        select_entity_type(&driver, "task").await?;
        let after_reload = headers(&driver).await?;
        assert_eq!(
            after_save, after_reload,
            "the saved columns did not come back after a reload"
        );

        // Put the workspace back the way it was found.
        // Queried rather than found: the button renders only once a type is selected and the
        // table has been rebuilt for it, and the reload above put the page back to "All types".
        driver
            .query(By::XPath("//button[contains(., 'Columns')]"))
            .wait(Duration::from_secs(10), Duration::from_millis(250))
            .first()
            .await?
            .click()
            .await?;
        driver
            .query(By::XPath("//button[contains(., 'Reset to default')]"))
            .wait(Duration::from_secs(10), Duration::from_millis(250))
            .first()
            .await?
            .click()
            .await?;
        tokio::time::sleep(Duration::from_secs(2)).await;
        WebDriverResult::Ok(())
    }
    .await;

    driver.quit().await.ok();
    result.unwrap();
}

/// A filter must narrow to rows that actually match, not merely to fewer rows.
///
/// Reads the filtered column's own cells rather than trusting the count: a filter that dropped every row, or one that matched the wrong field, would both look like a success to a row-count assertion.
#[tokio::test]
#[ignore = "needs a running server and chromedriver"]
async fn a_field_filter_narrows_to_rows_that_match() {
    let env = Env::from_env();
    let driver = browser(&env).await;

    let result = async {
        signed_in(&driver, &env).await?;
        let ws = workspace_id(&driver).await?;
        driver
            .goto(format!("{}/ws/{ws}/entities", env.base_url))
            .await?;
        select_entity_type(&driver, "task").await?;

        let Ok(filter) = driver
            .find(By::Css("select[aria-label=\"Filter by done\"]"))
            .await
        else {
            // The schema decides whether this filter exists at all, so its absence is a skip
            // rather than a failure. Reported, because a silent skip reads as a pass.
            eprintln!("skipped: this workspace's schema defines no boolean `done` field");
            return WebDriverResult::Ok(());
        };
        SelectElement::new(&filter)
            .await?
            .select_by_value("true")
            .await?;
        tokio::time::sleep(Duration::from_secs(2)).await;

        let rendered = headers(&driver).await?;
        let done_at = rendered
            .iter()
            .position(|h| h == "done")
            .expect("the `done` column must be visible to check what the filter did");

        let rows = driver.find_all(By::Css("tbody tr")).await?;
        assert!(!rows.is_empty(), "the filter matched nothing at all");
        for row in rows {
            let cells = row.find_all(By::Css("td")).await?;
            let text = cells[done_at].text().await?;
            assert_eq!(
                text.trim(),
                "Yes",
                "a row that does not match the filter was rendered"
            );
        }
        WebDriverResult::Ok(())
    }
    .await;

    driver.quit().await.ok();
    result.unwrap();
}
