//! Where people decide the tool calls that agents may not make on their own:
//! administrators any of them, members the ones their own access tokens made.
//! What a page says about a call is read by MyMCPs from the call itself: the
//! agent only ever hands over the link. (`approvals_controller.ts`)

use std::collections::{BTreeSet, HashMap};

use axum::Router;
use axum::extract::rejection::PathRejection;
use axum::extract::{OriginalUri, Path, State};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use http::header::ALLOW;
use http::{HeaderMap, Method, StatusCode};
use mymcps_core::models::{AccessToken, ApprovalRequest, Mcp, User};
use mymcps_core::{Db, Timestamp};
use mymcps_gateway::approvals::{ApprovalService, Decision, SavedApprovalSummary};
use mymcps_gateway::validators::approvals::{APPROVAL_DECISION, APPROVAL_PARAMS};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::sqlite::SqliteRow;
use sqlx::{QueryBuilder, Sqlite};

use crate::auth::CurrentUser;
use crate::error::{AppError, is_fetch};
use crate::forms::{FormState, refusal};
use crate::input::Input;
use crate::redirect::{redirect_back, redirect_to, with_query};
use crate::respond::{navigate, page};
use crate::routes::FeatureRoutes;
use crate::session::Session;
use crate::state::AppState;
use crate::views::approvals::{
    ApprovalPage, ApprovalView, ApprovalsPage, approval_page, approvals_page,
};
use crate::views::shell::PageContext;

const MAX_LISTED_PAST_REQUESTS: i64 = 50;

const NOT_FOUND: &str = "This approval request does not exist, was deleted after it expired, or belongs to the access token of another member";

/// Where sign-in sends a visitor who followed an approval link.
const RETURN_TO_KEY: &str = "approvalReturnTo";

pub fn routes() -> FeatureRoutes {
    FeatureRoutes {
        // The page behind an approval link. It sends guests to sign in and
        // brings them back, so it checks the session itself.
        open: Router::new().route("/approvals/{id}", get(show)),
        signed_in: Router::new()
            .route("/approvals", get(index))
            .route("/approvals/{id}", post(decide)),
        ..Default::default()
    }
}

/// The id a link names, or `None` when its last segment is not one.
fn link_id(path: Result<Path<String>, PathRejection>) -> Option<String> {
    let Path(segment) = path.ok()?;
    let link = APPROVAL_PARAMS.validate(&json!({ "id": segment })).ok()?;
    link.get("id")?.as_str().map(str::to_string)
}

/// The request a link names, or `None` when it matches none, or names a
/// request this user may not read: both get the same answer.
async fn find_request(
    state: &AppState,
    public_id: &str,
    user: &User,
) -> Result<Option<ApprovalRequest>, sqlx::Error> {
    let mut query = ApprovalService::visible_to(user);
    query
        .push(" and `approval_requests`.`public_id` = ")
        .push_bind(public_id)
        .push(" limit 1");
    query.build_query_as().fetch_optional(&*state.core.db).await
}

async fn rows_by_id<T>(
    db: &Db,
    select: &'static str,
    ids: BTreeSet<i64>,
    id_of: fn(&T) -> i64,
) -> Result<HashMap<i64, T>, sqlx::Error>
where
    T: for<'row> sqlx::FromRow<'row, SqliteRow> + Send + Unpin,
{
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let mut query = QueryBuilder::<Sqlite>::new(select);
    query.push(" where `id` in (");
    let mut listed = query.separated(", ");
    for id in ids {
        listed.push_bind(id);
    }
    query.push(")");
    let rows: Vec<T> = query.build_query_as().fetch_all(&**db).await?;
    Ok(rows.into_iter().map(|row| (id_of(&row), row)).collect())
}

/// What the pages show of some requests besides their own row: the MCP and
/// the access token of each, and who decided it.
struct Related {
    mcps: HashMap<i64, Mcp>,
    access_tokens: HashMap<i64, AccessToken>,
    deciders: HashMap<i64, User>,
}

impl Related {
    async fn of(db: &Db, requests: &[&ApprovalRequest]) -> Result<Self, sqlx::Error> {
        let ids = |id_of: fn(&ApprovalRequest) -> Option<i64>| -> BTreeSet<i64> {
            requests
                .iter()
                .filter_map(|request| id_of(request))
                .collect()
        };
        Ok(Self {
            mcps: rows_by_id(
                db,
                "select * from `mcps`",
                ids(|request| Some(request.mcp_id)),
                |mcp: &Mcp| mcp.id,
            )
            .await?,
            access_tokens: rows_by_id(
                db,
                "select * from `access_tokens`",
                ids(|request| Some(request.access_token_id)),
                |token: &AccessToken| token.id,
            )
            .await?,
            deciders: rows_by_id(
                db,
                "select * from `users`",
                ids(|request| request.decided_by),
                |user: &User| user.id,
            )
            .await?,
        })
    }

    /// The MCP and the access token of a request. A request goes with
    /// either of them, so both are there unless it just went.
    fn origin(&self, request: &ApprovalRequest) -> Option<(&Mcp, &AccessToken)> {
        Some((
            self.mcps.get(&request.mcp_id)?,
            self.access_tokens.get(&request.access_token_id)?,
        ))
    }

    fn view(
        &self,
        request: &ApprovalRequest,
        summary: Option<&SavedApprovalSummary>,
    ) -> Option<ApprovalView> {
        let (mcp, access_token) = self.origin(request)?;
        let decider = request
            .decided_by
            .and_then(|user_id| self.deciders.get(&user_id));
        Some(ApprovalView::of(
            request,
            mcp,
            access_token,
            decider,
            summary,
        ))
    }
}

/// `GET /approvals`
pub async fn index(
    State(state): State<AppState>,
    context: PageContext,
    CurrentUser(user): CurrentUser,
) -> Result<Response, AppError> {
    let db = &state.core.db;
    let now = Timestamp::now();
    // Requests made within the same second are listed the last one first.
    let mut waiting = ApprovalService::visible_to(&user);
    waiting
        .push(
            " and `approval_requests`.`status` = 'pending' and `approval_requests`.`expires_at` > ",
        )
        .push_bind(now)
        .push(" order by `approval_requests`.`created_at` desc, `approval_requests`.`id` desc");
    let waiting: Vec<ApprovalRequest> = waiting.build_query_as().fetch_all(&**db).await?;
    let mut past = ApprovalService::visible_to(&user);
    past.push(
        " and (`approval_requests`.`status` <> 'pending' or `approval_requests`.`expires_at` <= ",
    )
    .push_bind(now)
    .push(") order by `approval_requests`.`created_at` desc, `approval_requests`.`id` desc limit ")
    .push_bind(MAX_LISTED_PAST_REQUESTS);
    let past: Vec<ApprovalRequest> = past.build_query_as().fetch_all(&**db).await?;

    let all: Vec<&ApprovalRequest> = waiting.iter().chain(&past).collect();
    let related = Related::of(db, &all).await?;
    let approvals = &state.mcp_gateway.approvals;
    let views = |requests: &[ApprovalRequest]| -> Vec<ApprovalView> {
        requests
            .iter()
            .filter_map(|request| related.view(request, approvals.summary(request).as_ref()))
            .collect()
    };

    Ok(page(approvals_page(
        &context,
        &ApprovalsPage {
            waiting: &views(&waiting),
            past: &views(&past),
        },
    )))
}

/// `GET /approvals/{id}`
///
/// The page behind the link an agent was given. Whoever follows it signs in
/// first and comes back here: the link names a request and grants nothing.
pub async fn show(
    State(state): State<AppState>,
    method: Method,
    context: PageContext,
    session: Session,
    path: Result<Path<String>, PathRejection>,
) -> Result<Response, AppError> {
    // The router also sends HEAD requests here, which must not leave a return path behind.
    if method != Method::GET {
        return Ok((StatusCode::METHOD_NOT_ALLOWED, [(ALLOW, "GET")]).into_response());
    }

    let link = link_id(path);
    let Some(user) = &context.user else {
        if let Some(public_id) = &link {
            session.put(RETURN_TO_KEY, format!("/approvals/{public_id}"));
        }
        return Ok(redirect_to("/login"));
    };

    let not_found = || {
        session.flash("error", NOT_FOUND);
        redirect_to("/approvals")
    };
    let request = match &link {
        Some(public_id) => find_request(&state, public_id, user).await?,
        None => None,
    };
    let Some(request) = request else {
        return Ok(not_found());
    };
    let related = Related::of(&state.core.db, &[&request]).await?;
    let Some((mcp, access_token)) = related.origin(&request) else {
        return Ok(not_found());
    };

    // Read from the stored request, for someone who may read it: nothing
    // here comes from what the agent can still change.
    let approvals = &state.mcp_gateway.approvals;
    let summary = approvals.summary(&request);
    let arguments = approvals.arguments_text(&request);
    let Some(approval) = related.view(&request, summary.as_ref()) else {
        return Ok(not_found());
    };
    Ok(page(approval_page(
        &context,
        &ApprovalPage {
            approval: &approval,
            summary: summary.as_ref(),
            arguments: arguments.as_deref(),
            runnable: mcp.enabled && access_token.is_usable(),
        },
    )))
}

#[derive(Deserialize)]
struct Submitted {
    decision: String,
}

/// `POST /approvals/{id}`
pub async fn decide(
    State(state): State<AppState>,
    session: Session,
    CurrentUser(user): CurrentUser,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    path: Result<Path<String>, PathRejection>,
    Input(input): Input,
) -> Result<Response, AppError> {
    let request = match link_id(path) {
        Some(public_id) => find_request(&state, &public_id, &user).await?,
        None => None,
    };
    let Some(request) = request else {
        session.flash("error", NOT_FOUND);
        return Ok(navigate(&headers, &with_query("/approvals", uri.query())));
    };
    let request_page = format!("/approvals/{}", request.public_id);

    let submitted = APPROVAL_DECISION.validate_as::<Submitted>(&Value::Object(input.clone()));
    let decision = match submitted {
        Ok(submitted) => Decision::parse(&submitted.decision)
            .ok_or_else(|| AppError::internal("a decision the validator let through"))?,
        Err(error) => {
            let form = FormState::new(&refusal(error)?, &input);
            form.flash(&session);
            if let Some(message) = form.first_error() {
                session.flash("error", message);
            }
            return Ok(if is_fetch(&headers) {
                navigate(&headers, &request_page)
            } else {
                redirect_back(&headers, &request_page)
            });
        }
    };

    // Of two decisions made at once, the first to be written wins.
    let approvals = &state.mcp_gateway.approvals;
    if approvals.decide(&request, decision, &user).await? {
        session.flash(
            "success",
            match decision {
                Decision::Approve => "Approved. The agent can now run this call, once.",
                Decision::Deny => "Denied. The agent is told the call was refused.",
            },
        );
    } else {
        session.flash("error", "This request has expired or was already decided");
    }
    Ok(navigate(&headers, &with_query(&request_page, uri.query())))
}
