//! The API-Football adapter: the request it would build, the dry transport that answers it, and the
//! records a payload becomes.
//!
//! None of this opens a socket. [`ApiFootballAdapter`] knows the provider's URL shape, the header
//! its key travels in and how to read its JSON; [`SyncTransport`] is the seam where a real HTTP
//! client would sit, and the only implementation shipped is [`DrySyncTransport`], which answers
//! from an embedded fixture so a dry run is deterministic, free and offline.
//!
//! The records below keep every figure the provider may omit as `Option`. A payload that says
//! nothing about minutes produces `minutes_played: None`, never a zero that would later read as a
//! season of noughts.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fmt;

use chrono::{DateTime, Utc};
use serde::Deserialize;

use super::WorldSyncError;
use super::config::{ProviderKind, SyncMode, WorldSyncConfig};
use super::policy::{RouteFreshness, SyncRequestPlan, plan_request};

/// The provider's base URL. Only the route is configurable, so a test can point the adapter at a
/// local fixture server without changing what a production request looks like.
pub const API_FOOTBALL_BASE_URL: &str = "https://v3.football.api-sports.io";

/// The header API-Football reads the user's key from.
pub const API_KEY_HEADER: &str = "x-apisports-key";

/// `entity_links.entity_kind` for a player.
pub const ENTITY_KIND_PLAYER: &str = "player";
/// `entity_links.entity_kind` for a club.
pub const ENTITY_KIND_CLUB: &str = "club";
/// `entity_links.entity_kind` for a competition.
pub const ENTITY_KIND_COMPETITION: &str = "competition";

/// The world id the sync layer creates for an entity it has never seen.
///
/// Namespaced by provider so two providers cannot collide, and deterministic so re-running a sync
/// writes the same id rather than a fresh one. Nothing downstream reads it as a football fact — it
/// is a placeholder that a confirmed `entity_links` row may later point elsewhere.
pub fn provisional_world_id(
    provider: ProviderKind,
    entity_kind: &str,
    provider_id: &str,
) -> String {
    format!("{}-{entity_kind}-{}", provider.code(), provider_id.trim())
}

/// A competition a fixtures payload named.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CompetitionRecord {
    pub provider_id: String,
    pub name: String,
    pub short_name: Option<String>,
    pub country: Option<String>,
    /// The season the payload answered for.
    pub season: Option<u32>,
    /// The provider's own type (`league`, `cup`), when it said.
    pub kind: Option<String>,
    /// The provider does not rank competitions here; `None` stays `None` rather than "tier 1".
    pub tier: Option<u32>,
    pub confederation: Option<String>,
}

/// A club a fixtures or players payload named.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ClubRecord {
    pub provider_id: String,
    pub name: String,
    pub short_name: Option<String>,
    pub country: Option<String>,
    pub founded_year: Option<u32>,
    pub stadium_name: Option<String>,
}

/// A player and the current-season figures the provider reported for him.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PlayerRecord {
    pub provider_id: String,
    pub full_name: String,
    pub position: Option<String>,
    pub nationality: Option<String>,
    /// `YYYY-MM-DD`, when the provider said.
    pub birth_date: Option<String>,
    /// The provider id of the club he is registered with now.
    pub club_provider_id: Option<String>,
    /// API-Football does not rate players the way the game does, so this stays `None` until another
    /// source fills it. It is not derived from appearances or goals: that would be inventing a
    /// rating and calling it sourced.
    pub overall: Option<u8>,
    pub appearances: Option<u32>,
    pub minutes_played: Option<u32>,
    pub goals: Option<u32>,
    pub assists: Option<u32>,
    /// The provider's own match average, `None` when it did not rate him.
    pub rating: Option<f32>,
}

/// A provider identity a payload implies, ready for `world_sync::canonical`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EntityLinkRecord {
    pub provider: String,
    pub provider_entity_id: String,
    pub entity_kind: String,
    /// The provisional world id the sync layer would create for it.
    pub entity_id: String,
}

/// What one fixtures payload held.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FixtureBatch {
    pub competitions: Vec<CompetitionRecord>,
    pub clubs: Vec<ClubRecord>,
    pub links: Vec<EntityLinkRecord>,
}

/// A request the adapter built, with the key header it carries.
///
/// `Debug` redacts the key header, so logging a plan cannot leak the user's key.
#[derive(Clone, PartialEq, Eq)]
pub struct SyncRequest {
    pub provider: ProviderKind,
    pub route: String,
    pub endpoint: String,
    pub query: Vec<(String, String)>,
    pub headers: Vec<(String, String)>,
}

impl SyncRequest {
    /// The value sent for `name`, if any.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(header, _)| header.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

impl fmt::Debug for SyncRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let headers: Vec<(&str, &str)> = self
            .headers
            .iter()
            .map(|(name, value)| {
                let shown = if name.eq_ignore_ascii_case(API_KEY_HEADER) {
                    "<redacted>"
                } else {
                    value.as_str()
                };
                (name.as_str(), shown)
            })
            .collect();
        formatter
            .debug_struct("SyncRequest")
            .field("provider", &self.provider)
            .field("route", &self.route)
            .field("endpoint", &self.endpoint)
            .field("query", &self.query)
            .field("headers", &headers)
            .finish()
    }
}

/// A provider answer, as a transport returned it.
#[derive(Clone, PartialEq, Eq)]
pub struct SyncResponse {
    pub status: u16,
    pub body: String,
    pub etag: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
}

impl fmt::Debug for SyncResponse {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SyncResponse")
            .field("status", &self.status)
            .field("body_len", &self.body.len())
            .field("etag", &self.etag)
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

/// Where a request actually goes.
///
/// A real client plugs in here and nowhere else; the trait exists because the dry run is a genuine
/// second implementation, not a mock shaped around one caller.
pub trait SyncTransport {
    fn fetch(&self, request: &SyncRequest) -> Result<SyncResponse, WorldSyncError>;
}

const FIXTURES_FIXTURE: &str = r#"{
  "get": "fixtures",
  "parameters": {"league": "39", "season": "2026"},
  "errors": [],
  "results": 3,
  "paging": {"current": 1, "total": 1},
  "response": [
    {
      "fixture": {"id": 1001, "date": "2026-08-15T14:00:00+00:00", "status": {"short": "FT"}},
      "league": {"id": 39, "name": "Premier League", "country": "England", "season": 2026,
                 "round": "Regular Season - 1", "type": "league"},
      "teams": {
        "home": {"id": 50, "name": "Manchester City", "winner": true},
        "away": {"id": 42, "name": "Arsenal", "winner": false}
      },
      "goals": {"home": 2, "away": 1}
    },
    {
      "fixture": {"id": 1002, "date": "2026-08-22T14:00:00+00:00", "status": {"short": "FT"}},
      "league": {"id": 39, "name": "Premier League", "country": "England", "season": 2026,
                 "round": "Regular Season - 2", "type": "league"},
      "teams": {
        "home": {"id": 42, "name": "Arsenal", "winner": false},
        "away": {"id": 50, "name": "Manchester City", "winner": true}
      },
      "goals": {"home": 0, "away": 3}
    },
    {
      "fixture": {"id": 2031, "date": "2026-08-16T17:00:00+00:00", "status": {"short": "FT"}},
      "league": {"id": 203, "name": "Süper Lig", "country": "Turkey", "season": 2026,
                 "round": "Regular Season - 1", "type": "league"},
      "teams": {
        "home": {"id": 611, "name": "Galatasaray", "winner": false},
        "away": {"id": 607, "name": "Fenerbahçe", "winner": true}
      },
      "goals": {"home": 1, "away": 2}
    }
  ]
}"#;

const PLAYERS_FIXTURE: &str = r#"{
  "get": "players",
  "parameters": {"team": "50", "season": "2026"},
  "errors": [],
  "results": 2,
  "paging": {"current": 1, "total": 1},
  "response": [
    {
      "player": {
        "id": 874,
        "name": "Erling Haaland",
        "firstname": "Erling",
        "lastname": "Haaland",
        "age": 26,
        "birth": {"date": "2000-07-21", "place": "Leeds", "country": "England"},
        "nationality": "Norway",
        "height": "195 cm",
        "weight": "88 kg",
        "injured": false
      },
      "statistics": [
        {
          "team": {"id": 50, "name": "Manchester City"},
          "league": {"id": 39, "name": "Premier League", "country": "England", "season": 2026},
          "games": {"appearences": 11, "lineups": 11, "minutes": 990, "position": "Attacker",
                    "rating": "7.8", "captain": false},
          "goals": {"total": 13, "conceded": 0, "assists": 3, "saves": null},
          "cards": {"yellow": 1, "yellowred": 0, "red": 0}
        }
      ]
    },
    {
      "player": {
        "id": 875,
        "name": "Reserve Keeper",
        "firstname": "Reserve",
        "lastname": "Keeper",
        "age": 19,
        "birth": {"date": null, "place": null, "country": null},
        "nationality": null,
        "height": null,
        "weight": null,
        "injured": true
      },
      "statistics": [
        {
          "team": {"id": 50, "name": "Manchester City"},
          "league": {"id": 39, "name": "Premier League", "season": 2026},
          "games": {"appearences": null, "minutes": null, "position": "Goalkeeper",
                    "rating": null, "captain": false},
          "goals": {"total": null, "assists": null}
        }
      ]
    }
  ]
}"#;

/// A transport that answers from embedded fixtures: no socket, no key, no spend, and the same bytes
/// on every run.
#[derive(Debug)]
pub struct DrySyncTransport {
    fixtures: String,
    players: String,
    /// Every request it was asked, so a test can prove a plan sent exactly what it said it would.
    seen: RefCell<Vec<SyncRequest>>,
}

impl DrySyncTransport {
    /// The transport the embedded dry run uses.
    pub fn new() -> Self {
        Self::with_bodies(FIXTURES_FIXTURE, PLAYERS_FIXTURE)
    }

    /// A transport answering with `fixtures` and `players` bodies instead of the embedded pair.
    pub fn with_bodies(fixtures: impl Into<String>, players: impl Into<String>) -> Self {
        Self {
            fixtures: fixtures.into(),
            players: players.into(),
            seen: RefCell::new(Vec::new()),
        }
    }

    /// The requests this transport was asked to send, in order.
    pub fn requests(&self) -> Vec<SyncRequest> {
        self.seen.borrow().clone()
    }
}

impl Default for DrySyncTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl SyncTransport for DrySyncTransport {
    fn fetch(&self, request: &SyncRequest) -> Result<SyncResponse, WorldSyncError> {
        self.seen.borrow_mut().push(request.clone());
        let body = match request.route.as_str() {
            "fixtures" => self.fixtures.clone(),
            "players" => self.players.clone(),
            other => {
                return Err(WorldSyncError::Payload(format!(
                    "the dry transport has no fixture for route `{other}`"
                )));
            }
        };
        Ok(SyncResponse {
            status: 200,
            body,
            etag: Some(format!("dry-{}", request.route)),
            // The fixture says nothing about freshness, so the next run revalidates rather than
            // trusting an answer of unknown age.
            expires_at: None,
        })
    }
}

/// API-Football: the URL shape, the key header, and the JSON shapes its two routes answer with.
#[derive(Debug, Clone)]
pub struct ApiFootballAdapter {
    provider: ProviderKind,
    base_url: String,
}

impl ApiFootballAdapter {
    /// The production adapter.
    pub fn new() -> Self {
        Self::with_base_url(API_FOOTBALL_BASE_URL)
    }

    /// The same adapter against another host — a local fixture server, or a recorded proxy.
    pub fn with_base_url(base_url: impl Into<String>) -> Self {
        Self {
            provider: ProviderKind::ApiFootball,
            base_url: base_url.into(),
        }
    }

    /// The provider this adapter speaks for.
    pub fn provider(&self) -> ProviderKind {
        self.provider
    }

    /// The absolute URL of `route` with `query` appended and percent-encoded.
    pub fn endpoint(
        &self,
        route: &str,
        query: &[(String, String)],
    ) -> Result<String, WorldSyncError> {
        let mut url = url::Url::parse(&self.base_url).map_err(|error| {
            WorldSyncError::Payload(format!("base url `{}`: {error}", self.base_url))
        })?;
        url.set_path(&format!("/{}", route.trim().trim_start_matches('/')));
        {
            let mut pairs = url.query_pairs_mut();
            for (name, value) in query {
                pairs.append_pair(name, value);
            }
        }
        Ok(url.to_string())
    }

    /// Build the request a plan would send.
    ///
    /// `api_key` is `Option` because a dry plan is built before there is a key: a keyless request
    /// carries no key header at all, rather than an empty one.
    pub fn request(
        &self,
        route: &str,
        query: &[(String, String)],
        api_key: Option<&str>,
    ) -> Result<SyncRequest, WorldSyncError> {
        let endpoint = self.endpoint(route, query)?;
        let mut headers = vec![(String::from("accept"), String::from("application/json"))];
        if let Some(key) = api_key.map(str::trim).filter(|key| !key.is_empty()) {
            headers.push((String::from(API_KEY_HEADER), key.to_string()));
        }
        Ok(SyncRequest {
            provider: self.provider,
            route: route.trim().to_string(),
            endpoint,
            query: query.to_vec(),
            headers,
        })
    }

    /// Read a fixtures payload into competitions, clubs and the identities it implies.
    pub fn parse_fixtures(&self, body: &str) -> Result<FixtureBatch, WorldSyncError> {
        let payload: FixturesPayload = serde_json::from_str(body).map_err(payload_error)?;
        let mut competitions: BTreeMap<u64, CompetitionRecord> = BTreeMap::new();
        let mut clubs: BTreeMap<u64, ClubRecord> = BTreeMap::new();

        for entry in payload.response {
            let league = entry.league;
            competitions
                .entry(league.id)
                .or_insert_with(|| CompetitionRecord {
                    provider_id: league.id.to_string(),
                    name: league.name.clone(),
                    short_name: None,
                    country: league.country.clone(),
                    season: league.season,
                    kind: league.kind.clone(),
                    tier: None,
                    confederation: None,
                });
            for team in [entry.teams.home, entry.teams.away] {
                clubs.entry(team.id).or_insert_with(|| ClubRecord {
                    provider_id: team.id.to_string(),
                    name: team.name.clone(),
                    short_name: None,
                    country: None,
                    founded_year: team.founded,
                    stadium_name: team.stadium_name.clone(),
                });
            }
        }

        let mut links = Vec::with_capacity(competitions.len() + clubs.len());
        for provider_id in competitions.keys() {
            links.push(self.link(ENTITY_KIND_COMPETITION, &provider_id.to_string()));
        }
        for provider_id in clubs.keys() {
            links.push(self.link(ENTITY_KIND_CLUB, &provider_id.to_string()));
        }

        Ok(FixtureBatch {
            competitions: competitions.into_values().collect(),
            clubs: clubs.into_values().collect(),
            links,
        })
    }

    /// Read a players payload into one record per player, with every unreported figure left `None`.
    pub fn parse_players(&self, body: &str) -> Result<Vec<PlayerRecord>, WorldSyncError> {
        let payload: PlayersPayload = serde_json::from_str(body).map_err(payload_error)?;
        Ok(payload
            .response
            .into_iter()
            .map(player_record)
            .collect::<Vec<_>>())
    }

    /// The link a provider identity implies, pointing at the provisional world id.
    pub fn link(&self, entity_kind: &str, provider_id: &str) -> EntityLinkRecord {
        EntityLinkRecord {
            provider: self.provider.code().to_string(),
            provider_entity_id: provider_id.trim().to_string(),
            entity_kind: entity_kind.to_string(),
            entity_id: provisional_world_id(self.provider, entity_kind, provider_id),
        }
    }
}

impl Default for ApiFootballAdapter {
    fn default() -> Self {
        Self::new()
    }
}

/// One route a run wants, with what the sync remembers about its last answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncRoute {
    pub route: String,
    pub query: Vec<(String, String)>,
    pub freshness: Option<RouteFreshness>,
}

impl SyncRoute {
    /// A route with no history: the first run of a fresh world.
    pub fn first(route: impl Into<String>, query: Vec<(String, String)>) -> Self {
        Self {
            route: route.into(),
            query,
            freshness: None,
        }
    }
}

/// One planned step: what the policy says to do, and the request it would send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedStep {
    pub route: String,
    pub plan: SyncRequestPlan,
    /// `None` when the step sends nothing.
    pub request: Option<SyncRequest>,
}

/// What a run would do, without doing any of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncPlan {
    pub provider: ProviderKind,
    pub mode: SyncMode,
    pub steps: Vec<PlannedStep>,
}

/// How one step ended.
#[derive(Clone, PartialEq, Eq)]
pub enum SyncStepOutcome {
    /// The cached answer was still fresh; nothing was sent.
    SkippedFresh,
    /// The provider answered "not modified" to a conditional request.
    NotModified { etag: Option<String> },
    /// The provider answered with a body the caller may parse.
    Fetched {
        status: u16,
        etag: Option<String>,
        expires_at: Option<DateTime<Utc>>,
        body: String,
    },
    /// The provider answered with a status this skeleton does not read.
    Failed { status: u16 },
}

impl fmt::Debug for SyncStepOutcome {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SkippedFresh => formatter.write_str("SkippedFresh"),
            Self::NotModified { etag } => formatter
                .debug_struct("NotModified")
                .field("etag", etag)
                .finish(),
            Self::Fetched {
                status,
                etag,
                expires_at,
                body,
            } => formatter
                .debug_struct("Fetched")
                .field("status", status)
                .field("etag", etag)
                .field("expires_at", expires_at)
                .field("body_len", &body.len())
                .finish(),
            Self::Failed { status } => formatter
                .debug_struct("Failed")
                .field("status", status)
                .finish(),
        }
    }
}

/// One step's result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncStepResult {
    pub route: String,
    pub outcome: SyncStepOutcome,
}

/// What a run did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncExecution {
    pub plan: SyncPlan,
    pub results: Vec<SyncStepResult>,
}

impl SyncExecution {
    /// The body of a fetched step, for the parser the caller runs on it.
    pub fn body(&self, route: &str) -> Option<&str> {
        self.results
            .iter()
            .find_map(|result| match &result.outcome {
                SyncStepOutcome::Fetched { body, .. } if result.route == route => {
                    Some(body.as_str())
                }
                _ => None,
            })
    }
}

/// Turns a config and a list of wanted routes into a plan, and a plan into requests.
///
/// The orchestrator is API-Football specific because its adapter is; the mode it reports is the
/// config's *effective* mode, so a plan can never claim to be live when the config blocked it.
pub struct SyncOrchestrator {
    config: WorldSyncConfig,
    adapter: ApiFootballAdapter,
}

impl SyncOrchestrator {
    /// An orchestrator for `config`.
    pub fn new(config: WorldSyncConfig) -> Self {
        Self {
            config,
            adapter: ApiFootballAdapter::new(),
        }
    }

    /// The config this orchestrator plans with.
    pub fn config(&self) -> &WorldSyncConfig {
        &self.config
    }

    /// What each route would do at `now`, without sending anything.
    pub fn plan(
        &self,
        routes: &[SyncRoute],
        now: DateTime<Utc>,
    ) -> Result<SyncPlan, WorldSyncError> {
        let api_key = (self.config.effective_mode() == SyncMode::Live)
            .then(|| self.config.api_key())
            .flatten();
        let mut steps = Vec::with_capacity(routes.len());
        for route in routes {
            let plan = plan_request(now, route.freshness.as_ref());
            let request = match &plan {
                SyncRequestPlan::SkipFresh => None,
                SyncRequestPlan::FetchConditional { if_none_match } => {
                    let mut request =
                        self.adapter
                            .request(&route.route, &route.query, api_key.as_deref())?;
                    request
                        .headers
                        .push((String::from("if-none-match"), if_none_match.clone()));
                    Some(request)
                }
                SyncRequestPlan::FetchFull => Some(self.adapter.request(
                    &route.route,
                    &route.query,
                    api_key.as_deref(),
                )?),
            };
            steps.push(PlannedStep {
                route: route.route.clone(),
                plan,
                request,
            });
        }
        Ok(SyncPlan {
            provider: self.config.provider,
            mode: self.config.effective_mode(),
            steps,
        })
    }

    /// Send what the plan says through `transport`.
    ///
    /// Nothing here checks or enables a network: the only transport in the tree answers from a
    /// fixture, and a real client would be a second implementation plugged in by the app.
    pub fn run(
        &self,
        routes: &[SyncRoute],
        now: DateTime<Utc>,
        transport: &dyn SyncTransport,
    ) -> Result<SyncExecution, WorldSyncError> {
        let plan = self.plan(routes, now)?;
        let mut results = Vec::with_capacity(plan.steps.len());
        for step in &plan.steps {
            let outcome = match &step.request {
                None => SyncStepOutcome::SkippedFresh,
                Some(request) => outcome_of(transport.fetch(request)?, &step.plan),
            };
            results.push(SyncStepResult {
                route: step.route.clone(),
                outcome,
            });
        }
        Ok(SyncExecution { plan, results })
    }
}

fn outcome_of(response: SyncResponse, plan: &SyncRequestPlan) -> SyncStepOutcome {
    if response.status == 304 && matches!(plan, SyncRequestPlan::FetchConditional { .. }) {
        return SyncStepOutcome::NotModified {
            etag: response.etag,
        };
    }
    if (200..300).contains(&response.status) {
        return SyncStepOutcome::Fetched {
            status: response.status,
            etag: response.etag,
            expires_at: response.expires_at,
            body: response.body,
        };
    }
    SyncStepOutcome::Failed {
        status: response.status,
    }
}

fn player_record(entry: PlayerEntry) -> PlayerRecord {
    let info = entry.player;
    let statistics = entry.statistics.into_iter().next();
    let games = statistics.as_ref().and_then(|stat| stat.games.as_ref());
    let goals = statistics.as_ref().and_then(|stat| stat.goals.as_ref());

    PlayerRecord {
        provider_id: info.id.to_string(),
        full_name: display_name(&info),
        position: games.and_then(|games| games.position.clone()),
        nationality: non_blank(info.nationality),
        birth_date: info
            .birth
            .and_then(|birth| birth.date)
            .and_then(|date| non_blank(Some(date))),
        club_provider_id: statistics
            .as_ref()
            .and_then(|stat| stat.team.as_ref())
            .and_then(|team| team.id)
            .map(|id| id.to_string()),
        overall: None,
        appearances: games.and_then(|games| games.appearences),
        minutes_played: games.and_then(|games| games.minutes),
        goals: goals.and_then(|goals| goals.total),
        assists: goals.and_then(|goals| goals.assists),
        rating: games
            .and_then(|games| games.rating.as_deref())
            .and_then(|rating| rating.trim().parse::<f32>().ok()),
    }
}

fn display_name(info: &PlayerInfo) -> String {
    let name = info.name.trim();
    if !name.is_empty() {
        return name.to_string();
    }
    format!(
        "{} {}",
        info.firstname.as_deref().unwrap_or_default().trim(),
        info.lastname.as_deref().unwrap_or_default().trim()
    )
    .trim()
    .to_string()
}

fn non_blank(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn payload_error(error: serde_json::Error) -> WorldSyncError {
    WorldSyncError::Payload(error.to_string())
}

#[derive(Deserialize)]
struct FixturesPayload {
    #[serde(default)]
    response: Vec<FixtureEntry>,
}

#[derive(Deserialize)]
struct FixtureEntry {
    league: LeagueInfo,
    teams: TeamsInfo,
}

#[derive(Deserialize)]
struct LeagueInfo {
    id: u64,
    name: String,
    #[serde(default)]
    country: Option<String>,
    #[serde(default)]
    season: Option<u32>,
    #[serde(default, rename = "type")]
    kind: Option<String>,
}

#[derive(Deserialize)]
struct TeamsInfo {
    home: TeamInfo,
    away: TeamInfo,
}

#[derive(Deserialize)]
struct TeamInfo {
    id: u64,
    name: String,
    #[serde(default)]
    founded: Option<u32>,
    #[serde(default)]
    stadium_name: Option<String>,
}

#[derive(Deserialize)]
struct PlayersPayload {
    #[serde(default)]
    response: Vec<PlayerEntry>,
}

#[derive(Deserialize)]
struct PlayerEntry {
    player: PlayerInfo,
    #[serde(default)]
    statistics: Vec<PlayerStatistics>,
}

#[derive(Deserialize)]
struct PlayerInfo {
    id: u64,
    #[serde(default)]
    name: String,
    #[serde(default)]
    firstname: Option<String>,
    #[serde(default)]
    lastname: Option<String>,
    #[serde(default)]
    nationality: Option<String>,
    #[serde(default)]
    birth: Option<BirthInfo>,
}

#[derive(Deserialize)]
struct BirthInfo {
    #[serde(default)]
    date: Option<String>,
}

#[derive(Deserialize)]
struct PlayerStatistics {
    #[serde(default)]
    team: Option<TeamRef>,
    #[serde(default)]
    games: Option<GamesInfo>,
    #[serde(default)]
    goals: Option<GoalsInfo>,
}

#[derive(Deserialize)]
struct TeamRef {
    /// Absent is absent: a missing team id must not become club `0`.
    #[serde(default)]
    id: Option<u64>,
}

#[derive(Deserialize)]
struct GamesInfo {
    /// API-Football spells it this way; matching the payload matters more than the spelling.
    #[serde(default)]
    appearences: Option<u32>,
    #[serde(default)]
    minutes: Option<u32>,
    #[serde(default)]
    position: Option<String>,
    /// A decimal string in the payload (`"7.8"`), null when nothing was rated.
    #[serde(default)]
    rating: Option<String>,
}

#[derive(Deserialize)]
struct GoalsInfo {
    #[serde(default)]
    total: Option<u32>,
    #[serde(default)]
    assists: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world_sync::policy::expires_at_from;

    fn at(seconds: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(seconds, 0).expect("a representable instant")
    }

    fn orchestrator(config: WorldSyncConfig) -> SyncOrchestrator {
        SyncOrchestrator::new(config)
    }

    /// Given a key
    /// When the adapter builds a request
    /// Then the key travels in the provider's header and appears in no `Debug` output.
    #[test]
    fn a_request_carries_the_key_header_and_never_prints_it() {
        let adapter = ApiFootballAdapter::new();
        let request = adapter
            .request(
                "fixtures",
                &[(String::from("league"), String::from("39"))],
                Some("  sk-secret-123  "),
            )
            .expect("a request is buildable");

        assert_eq!(request.header(API_KEY_HEADER), Some("sk-secret-123"));
        assert_eq!(
            request.endpoint,
            "https://v3.football.api-sports.io/fixtures?league=39"
        );
        let printed = format!("{request:?}");
        assert!(!printed.contains("sk-secret-123"), "{printed}");
        assert!(printed.contains("<redacted>"), "{printed}");
    }

    /// Given no key, as a fresh dry plan has
    /// When a request is built
    /// Then it carries no key header at all rather than an empty one.
    #[test]
    fn a_keyless_dry_request_carries_no_key_header() {
        let adapter = ApiFootballAdapter::new();
        let request = adapter
            .request("players", &[], None)
            .expect("a dry request is buildable");
        assert_eq!(request.header(API_KEY_HEADER), None);

        let blank = adapter
            .request("players", &[], Some("   "))
            .expect("a blank key is still a dry request");
        assert_eq!(blank.header(API_KEY_HEADER), None);
    }

    /// Given a query with a space and an apostrophe
    /// When the endpoint is built
    /// Then the values are percent-encoded so the provider parses them, not the transport.
    #[test]
    fn query_values_are_percent_encoded() {
        let adapter = ApiFootballAdapter::with_base_url("https://example.test/");
        let endpoint = adapter
            .endpoint(
                "/players",
                &[
                    (String::from("search"), String::from("O'Brien")),
                    (String::from("season"), String::from("2026")),
                ],
            )
            .expect("a valid base url");

        assert_eq!(
            endpoint,
            "https://example.test/players?search=O%27Brien&season=2026"
        );
    }

    /// Given the embedded fixtures
    /// When a dry transport is asked for both routes
    /// Then it answers offline, records what it was asked, and refuses a route it has no fixture
    ///      for.
    #[test]
    fn a_dry_transport_answers_from_fixtures_and_records_each_request() {
        let adapter = ApiFootballAdapter::new();
        let transport = DrySyncTransport::new();

        let fixtures = transport
            .fetch(&adapter.request("fixtures", &[], None).expect("request"))
            .expect("the fixtures fixture is embedded");
        let players = transport
            .fetch(&adapter.request("players", &[], None).expect("request"))
            .expect("the players fixture is embedded");

        assert_eq!(fixtures.status, 200);
        assert!(fixtures.body.contains("\"Premier League\""));
        assert_eq!(players.status, 200);
        assert!(players.body.contains("Erling Haaland"));
        assert_eq!(transport.requests().len(), 2);
        assert_eq!(transport.requests()[0].route, "fixtures");

        let missing = transport.fetch(&adapter.request("odds", &[], None).expect("request"));
        assert!(matches!(missing, Err(WorldSyncError::Payload(_))));
        assert_eq!(
            transport.requests().len(),
            3,
            "a refused route was still an attempt"
        );
    }

    /// Given a fixtures payload that names one competition twice and one club in both fixtures
    /// When it is parsed
    /// Then each competition and club appears once and each implies one link.
    #[test]
    fn a_fixtures_payload_parses_into_deduped_competitions_clubs_and_links() {
        let adapter = ApiFootballAdapter::new();
        let batch = adapter
            .parse_fixtures(FIXTURES_FIXTURE)
            .expect("the embedded fixtures parse");

        assert_eq!(
            batch
                .competitions
                .iter()
                .map(|competition| competition.name.as_str())
                .collect::<Vec<_>>(),
            vec!["Premier League", "Süper Lig"]
        );
        assert_eq!(
            batch
                .clubs
                .iter()
                .map(|club| club.name.as_str())
                .collect::<Vec<_>>(),
            vec!["Arsenal", "Manchester City", "Fenerbahçe", "Galatasaray"],
            "clubs are keyed by provider id, not by fixture"
        );
        let premier = &batch.competitions[0];
        assert_eq!(premier.provider_id, "39");
        assert_eq!(premier.country.as_deref(), Some("England"));
        assert_eq!(premier.season, Some(2026));
        assert_eq!(premier.kind.as_deref(), Some("league"));
        assert_eq!(premier.tier, None, "the provider never ranks competitions");

        assert_eq!(batch.links.len(), 6, "two competitions and four clubs");
        assert!(batch.links.iter().any(|link| {
            link.entity_kind == ENTITY_KIND_CLUB
                && link.provider_entity_id == "50"
                && link.entity_id == "api-football-club-50"
        }));
        assert!(batch.links.iter().any(|link| {
            link.entity_kind == ENTITY_KIND_COMPETITION
                && link.entity_id == "api-football-competition-39"
        }));
    }

    /// Given a players payload with one fully reported player and one the provider left blank
    /// When it is parsed
    /// Then the unreported figures are `None`, never zero, and no rating is invented.
    #[test]
    fn a_player_without_reported_statistics_keeps_none_not_zero() {
        let adapter = ApiFootballAdapter::new();
        let players = adapter
            .parse_players(PLAYERS_FIXTURE)
            .expect("the embedded players parse");
        assert_eq!(players.len(), 2);

        let rated = &players[0];
        assert_eq!(rated.full_name, "Erling Haaland");
        assert_eq!(rated.birth_date.as_deref(), Some("2000-07-21"));
        assert_eq!(rated.nationality.as_deref(), Some("Norway"));
        assert_eq!(rated.club_provider_id.as_deref(), Some("50"));
        assert_eq!(rated.position.as_deref(), Some("Attacker"));
        assert_eq!(rated.appearances, Some(11));
        assert_eq!(rated.minutes_played, Some(990));
        assert_eq!(rated.goals, Some(13));
        assert_eq!(rated.assists, Some(3));
        assert_eq!(rated.rating, Some(7.8));
        assert_eq!(
            rated.overall, None,
            "a provider with no overall does not get one invented"
        );

        let unknown = &players[1];
        assert_eq!(unknown.full_name, "Reserve Keeper");
        assert_eq!(unknown.birth_date, None);
        assert_eq!(unknown.nationality, None);
        assert_eq!(unknown.appearances, None);
        assert_eq!(unknown.minutes_played, None);
        assert_eq!(unknown.goals, None);
        assert_eq!(unknown.assists, None);
        assert_eq!(unknown.rating, None);
    }

    /// Given a payload that is not the shape the adapter expects
    /// When it is parsed
    /// Then it is an error naming the payload, not an empty set of rows.
    #[test]
    fn an_unreadable_payload_is_an_error_not_an_empty_batch() {
        let adapter = ApiFootballAdapter::new();
        assert!(matches!(
            adapter.parse_fixtures("{\"response\": \"nope\"}"),
            Err(WorldSyncError::Payload(_))
        ));
        assert!(matches!(
            adapter.parse_players("not json at all"),
            Err(WorldSyncError::Payload(_))
        ));
    }

    /// Given a dry config with no key
    /// When a plan is made for a route with no history
    /// Then the plan is dry, asks for the whole payload, and shows a keyless request.
    #[test]
    fn a_dry_plan_without_a_key_still_shows_the_request_it_would_send() {
        let plan = orchestrator(WorldSyncConfig::default())
            .plan(
                &[SyncRoute::first(
                    "fixtures",
                    vec![(String::from("league"), String::from("39"))],
                )],
                at(1_000),
            )
            .expect("a dry plan cannot fail on a valid base url");

        assert_eq!(plan.mode, SyncMode::DryRun);
        assert_eq!(plan.provider, ProviderKind::ApiFootball);
        assert_eq!(plan.steps.len(), 1);
        assert_eq!(plan.steps[0].plan, SyncRequestPlan::FetchFull);
        let request = plan.steps[0]
            .request
            .as_ref()
            .expect("a fetch step shows its request");
        assert_eq!(request.header(API_KEY_HEADER), None);
        assert!(request.endpoint.contains("league=39"));
    }

    /// Given a route whose cached answer is fresh, and one whose answer expired with an ETag
    /// When a plan is made
    /// Then the fresh route is skipped and the expired one is revalidated with its ETag.
    #[test]
    fn a_plan_skips_fresh_routes_and_revalidates_stale_ones() {
        let config = WorldSyncConfig::new(ProviderKind::ApiFootball)
            .with_key(crate::world_sync::ApiKeyHandle::supplied("sk-test"));
        let fresh = RouteFreshness {
            etag: Some(String::from("\"v1\"")),
            expires_at: Some(expires_at_from(at(1_000), 3_600)),
            fetched_at: at(1_000),
        };
        let stale = RouteFreshness {
            etag: Some(String::from("\"v2\"")),
            expires_at: Some(at(1_500)),
            fetched_at: at(1_000),
        };
        let routes = vec![
            SyncRoute {
                route: String::from("fixtures"),
                query: Vec::new(),
                freshness: Some(fresh),
            },
            SyncRoute {
                route: String::from("players"),
                query: Vec::new(),
                freshness: Some(stale),
            },
        ];

        let plan = orchestrator(config)
            .plan(&routes, at(2_000))
            .expect("a valid base url");

        assert_eq!(plan.mode, SyncMode::DryRun);
        assert_eq!(plan.steps[0].plan, SyncRequestPlan::SkipFresh);
        assert!(plan.steps[0].request.is_none());
        assert_eq!(
            plan.steps[1].plan,
            SyncRequestPlan::FetchConditional {
                if_none_match: String::from("\"v2\"")
            }
        );
        let conditional_req = plan.steps[1]
            .request
            .as_ref()
            .expect("conditional step must carry a request");
        assert_eq!(conditional_req.header("if-none-match"), Some("\"v2\""));
        assert_eq!(
            conditional_req.header(API_KEY_HEADER),
            None,
            "DryRun must never embed a live key header even when one is supplied"
        );
    }

    /// Given a dry run over both routes
    /// When it executes against the dry transport
    /// Then each step reports a fetched body, and the fixtures body parses into competitions.
    #[test]
    fn a_dry_run_fetches_each_route_and_its_bodies_parse() {
        let execution = orchestrator(WorldSyncConfig::default())
            .run(
                &[
                    SyncRoute::first("fixtures", Vec::new()),
                    SyncRoute::first("players", Vec::new()),
                ],
                at(1_000),
                &DrySyncTransport::new(),
            )
            .expect("a dry run is offline");

        assert_eq!(execution.results.len(), 2);
        assert!(matches!(
            execution.results[0].outcome,
            SyncStepOutcome::Fetched { status: 200, .. }
        ));
        let body = execution
            .body("fixtures")
            .expect("the fixtures body is kept");
        let batch = ApiFootballAdapter::new()
            .parse_fixtures(body)
            .expect("a fetched body parses");
        assert_eq!(batch.competitions.len(), 2);
        assert!(execution.body("players").is_some());
    }

    /// Given a conditional step the provider answers 304 to
    /// When the run classifies it
    /// Then nothing is parsed as a payload: the step is "not modified".
    #[test]
    fn a_not_modified_answer_is_reported_without_a_body() {
        struct NotModified;

        impl SyncTransport for NotModified {
            fn fetch(&self, _request: &SyncRequest) -> Result<SyncResponse, WorldSyncError> {
                Ok(SyncResponse {
                    status: 304,
                    body: String::new(),
                    etag: Some(String::from("\"v2\"")),
                    expires_at: None,
                })
            }
        }

        let stale = RouteFreshness {
            etag: Some(String::from("\"v2\"")),
            expires_at: Some(at(1_500)),
            fetched_at: at(1_000),
        };
        let execution = orchestrator(WorldSyncConfig::default())
            .run(
                &[SyncRoute {
                    route: String::from("fixtures"),
                    query: Vec::new(),
                    freshness: Some(stale),
                }],
                at(2_000),
                &NotModified,
            )
            .expect("a not-modified run succeeds");

        assert_eq!(
            execution.results[0].outcome,
            SyncStepOutcome::NotModified {
                etag: Some(String::from("\"v2\""))
            }
        );
        assert!(execution.body("fixtures").is_none());
    }

    /// Given a provider answer with a status the skeleton does not read
    /// When the run classifies it
    /// Then the step is failed and no body is exposed.
    #[test]
    fn an_error_status_is_a_failed_step() {
        struct Unavailable;

        impl SyncTransport for Unavailable {
            fn fetch(&self, _request: &SyncRequest) -> Result<SyncResponse, WorldSyncError> {
                Ok(SyncResponse {
                    status: 429,
                    body: String::from("quota"),
                    etag: None,
                    expires_at: None,
                })
            }
        }

        let execution = orchestrator(WorldSyncConfig::default())
            .run(
                &[SyncRoute::first("fixtures", Vec::new())],
                at(1_000),
                &Unavailable,
            )
            .expect("an error status is still a completed run");

        assert_eq!(
            execution.results[0].outcome,
            SyncStepOutcome::Failed { status: 429 }
        );
    }

    /// Given a provider identity with spaces around it
    /// When a provisional world id is minted
    /// Then it is namespaced by provider and trimmed.
    #[test]
    fn a_provisional_world_id_is_namespaced_and_trimmed() {
        assert_eq!(
            provisional_world_id(ProviderKind::ApiFootball, ENTITY_KIND_PLAYER, " 874 "),
            "api-football-player-874"
        );
        assert_eq!(
            provisional_world_id(ProviderKind::LocalFixture, ENTITY_KIND_CLUB, "50"),
            "local-fixture-club-50"
        );
    }
}
