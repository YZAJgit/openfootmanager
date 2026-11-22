//! The read-side contract of the world database (`world.db`).
//!
//! `world.db` is a separate, read-mostly SQLite file from a career save: identity rows, season
//! statistics and membership periods for a world that can hold millions of players. The SQL
//! lives in `db::world_db`, because persistence belongs to the `db` crate and `ofm_core` links no
//! SQLite. This module is the half both sides share: the filters, sort keys and result shapes a
//! caller asks with, plus the two pure helpers the importer and the query layer must agree on —
//! name folding and the FTS5 query it feeds.
//!
//! Nothing here reads a file or a database, which is what lets a career, an agent tool or a test
//! build a query without opening one. The three reads the browse screens use —
//! `search_players_page`, `club_squad_page` and `get_player_profile` — are executed by
//! `db::world_db::WorldDatabase` against these types, because SQLite is the `db` crate's
//! dependency and `ofm_core` deliberately does not link it.

use domain::player::Position;
use serde::{Deserialize, Serialize};

/// The page size a caller gets when it does not ask for one.
pub const DEFAULT_PAGE_SIZE: u32 = 50;

/// The largest page the query layer will return, whatever a caller asks for.
///
/// A page is something a screen renders, not an export: without a ceiling one request can pull a
/// hundred thousand rows into memory and undo the point of paging.
pub const MAX_PAGE_SIZE: u32 = 200;

/// An offset/limit window over a sorted result set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Page {
    pub offset: u64,
    /// Clamped to `1..=MAX_PAGE_SIZE` when built through [`Page::new`].
    pub limit: u32,
}

impl Page {
    /// A window with `limit` clamped into the supported range.
    pub fn new(offset: u64, limit: u32) -> Self {
        Self {
            offset,
            limit: clamp_page_size(limit),
        }
    }

    /// The first page of `limit` rows.
    pub fn first(limit: u32) -> Self {
        Self::new(0, limit)
    }
}

impl Default for Page {
    fn default() -> Self {
        Self::first(DEFAULT_PAGE_SIZE)
    }
}

/// Clamp a requested page size into the range a screen can render.
///
/// Shared with keyset paging, so the two paging styles cannot disagree on what a page is.
pub fn clamp_page_size(limit: u32) -> u32 {
    limit.clamp(1, MAX_PAGE_SIZE)
}

/// Whether a position filter names one exact position or a whole outfield group.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PositionFilter {
    pub position: Position,
    /// `true` matches every player whose group is this position's group — asking for `Defender`
    /// finds centre-backs and full-backs alike; asking for `CenterBack` with this `false` finds
    /// centre-backs only.
    pub match_group: bool,
}

impl PositionFilter {
    /// One exact position (`Striker`, `LeftBack`, ...).
    pub fn exact(position: Position) -> Self {
        Self {
            position,
            match_group: false,
        }
    }

    /// Every position in `position`'s group. A coarse position (`Defender`) is its own group.
    pub fn group(position: Position) -> Self {
        Self {
            position,
            match_group: true,
        }
    }

    /// The position code a coarse or exact filter stores.
    pub fn code(&self) -> &'static str {
        position_code(&self.position)
    }

    /// The group code when the filter is a group filter.
    pub fn group_code(&self) -> Option<&'static str> {
        self.match_group
            .then(|| position_group_code(&self.position.to_group_position()))
    }
}

/// One column a player page can be ordered by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlayerSortColumn {
    Overall,
    Potential,
    Name,
    BirthYear,
}

/// A sort column and its direction. Ties are always broken by player id so paging is stable.
/// A column the source never filled in — an unsourced birth year, an unrated player — leads an
/// ascending browse and trails a descending one, because NULL is "never said", not the lowest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayerSort {
    pub column: PlayerSortColumn,
    pub descending: bool,
}

impl PlayerSort {
    pub fn ascending(column: PlayerSortColumn) -> Self {
        Self {
            column,
            descending: false,
        }
    }

    pub fn descending(column: PlayerSortColumn) -> Self {
        Self {
            column,
            descending: true,
        }
    }
}

impl Default for PlayerSort {
    /// Strongest first, the order every squad screen opens with.
    fn default() -> Self {
        Self::descending(PlayerSortColumn::Overall)
    }
}

/// The filters of a player search. Every field is optional; the empty search browses the world.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayerSearch {
    /// Free text over the player's name and his current club's name. Missing or blank means no
    /// name constraint.
    pub name: Option<String>,
    /// An exact `world_clubs.id`.
    pub club_id: Option<String>,
    pub position: Option<PositionFilter>,
    /// An exact nationality code (`ENG`, `TR`, ...).
    pub nationality: Option<String>,
    pub overall_min: Option<u8>,
    pub overall_max: Option<u8>,
    pub sort: PlayerSort,
}

impl PlayerSearch {
    /// The FTS5 expression this search's name becomes, or `None` for a blank name.
    pub fn name_query(&self) -> Option<String> {
        self.name.as_deref().and_then(fts_query)
    }
}

/// A row of a player page: the identity fields a list screen shows, not the whole profile.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlayerSummary {
    pub id: String,
    pub full_name: String,
    /// The exact position code (`Striker`), as stored.
    pub position: String,
    /// The group code (`forward`), as stored.
    pub position_group: String,
    pub nationality: String,
    /// `YYYY-MM-DD`, `None` when the source did not say.
    pub birth_date: Option<String>,
    pub birth_year: Option<i32>,
    pub current_club_id: Option<String>,
    pub current_club_name: Option<String>,
    /// `None` when the source did not say — never a stand-in zero.
    pub overall: Option<u8>,
    pub potential: Option<u8>,
}

/// One page of players plus the size of the whole filtered set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlayerPage {
    pub players: Vec<PlayerSummary>,
    pub total: u64,
    pub offset: u64,
    pub limit: u32,
}

/// The value of the sort column on a row a keyset page stopped at.
///
/// A `Number(None)` is a NULL in an indexed rating or birth-year column: it is a position in the
/// order (NULLs lead an ascending browse and trail a descending one), not a zero.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlayerSortValue {
    /// `overall`, `potential` or `birth_year`.
    Number(Option<i32>),
    /// The folded name a `Name` sort orders by (`fold_name` of the display name, the same form the
    /// importer stores in `name_normalized`).
    Text(String),
}

/// A seek position: the last row a caller saw, in the order it saw it.
///
/// Keyset paging asks for the rows *after* this position instead of counting an `OFFSET` past it,
/// so page ten thousand costs the same as page two. [`PlayerKeysetCursor::after`] is the only
/// constructor a caller needs: it reads the value from the row it was given, which is why the
/// position cannot drift from the sort that produced it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayerKeysetCursor {
    /// The order this position belongs to. A cursor is refused against any other order, because a
    /// position read in the wrong direction would silently skip or repeat rows.
    pub sort: PlayerSort,
    pub sort_value: PlayerSortValue,
    /// The row's id, the tie-break every order ends with.
    pub player_id: String,
}

impl PlayerKeysetCursor {
    /// The position immediately after `player`, in `sort`'s order.
    pub fn after(sort: PlayerSort, player: &PlayerSummary) -> Self {
        let sort_value = match sort.column {
            PlayerSortColumn::Overall => PlayerSortValue::Number(player.overall.map(i32::from)),
            PlayerSortColumn::Potential => PlayerSortValue::Number(player.potential.map(i32::from)),
            PlayerSortColumn::BirthYear => PlayerSortValue::Number(player.birth_year),
            // Resolved to the stored form, not the caller's spelling, so a name cursor compares
            // against the index exactly as the importer wrote it.
            PlayerSortColumn::Name => PlayerSortValue::Text(fold_name(&player.full_name)),
        };
        Self {
            sort,
            sort_value,
            player_id: player.id.clone(),
        }
    }
}

/// One keyset page of players plus the seek position for the next one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlayerKeysetPage {
    pub players: Vec<PlayerSummary>,
    /// The size of the whole filtered set, counted only on the first page.
    ///
    /// A walk that re-counted on every page would pay a full scan per page — exactly the cost
    /// seeking removes. A later page therefore reports `None`, and the caller carries the first
    /// page's total forward. A zero here would claim an empty world.
    pub total: Option<u64>,
    pub limit: u32,
    /// Where the next page starts, or `None` when this page ended the set.
    ///
    /// A page that filled its window reports a cursor even when nothing follows it, so a caller
    /// may see one empty page at the end; paging never stops early, which is the error worth
    /// avoiding.
    pub next_cursor: Option<PlayerKeysetCursor>,
}

/// A club a player belonged to over a validity interval.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MembershipPeriod {
    /// `None` when the club is not one this world defines (an authored career row by name only).
    pub club_id: Option<String>,
    pub club_name: String,
    /// `YYYY-MM-DD`; `None` when the period's start is unknown.
    pub valid_from: Option<String>,
    /// `YYYY-MM-DD`; `None` while the period is open (the current club).
    pub valid_to: Option<String>,
    /// `current` or `career`.
    pub kind: String,
}

/// One season of statistics in one competition. Every figure is optional because a source that
/// did not report a value must not read as a zero.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SeasonStatLine {
    pub season: u32,
    /// Empty for the player's whole season (all competitions), otherwise the competition id.
    pub competition_id: String,
    pub club_id: Option<String>,
    pub appearances: Option<u32>,
    pub minutes_played: Option<u32>,
    pub goals: Option<u32>,
    pub assists: Option<u32>,
    pub clean_sheets: Option<u32>,
    pub yellow_cards: Option<u32>,
    pub red_cards: Option<u32>,
    pub avg_rating: Option<f32>,
}

/// A player's full profile: identity, stored attributes, membership history and season history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlayerProfile {
    pub player: PlayerSummary,
    /// The attributes blob exactly as stored (JSON object text).
    pub attributes_json: String,
    pub footedness: Option<String>,
    pub weak_foot: Option<u8>,
    /// Newest first.
    pub memberships: Vec<MembershipPeriod>,
    /// Newest season first, then competition id.
    pub season_stats: Vec<SeasonStatLine>,
}

/// The stable code a position is stored under: its variant name.
///
/// Exhaustive on purpose: a new `Position` variant stops the build here rather than silently
/// landing in a world database with no code. `position_json` in the save path is not a substitute,
/// because that spelling changes with serde settings and this one must not.
pub fn position_code(position: &Position) -> &'static str {
    match position {
        Position::Goalkeeper => "Goalkeeper",
        Position::Defender => "Defender",
        Position::Midfielder => "Midfielder",
        Position::Forward => "Forward",
        Position::RightBack => "RightBack",
        Position::CenterBack => "CenterBack",
        Position::LeftBack => "LeftBack",
        Position::RightWingBack => "RightWingBack",
        Position::LeftWingBack => "LeftWingBack",
        Position::DefensiveMidfielder => "DefensiveMidfielder",
        Position::CentralMidfielder => "CentralMidfielder",
        Position::AttackingMidfielder => "AttackingMidfielder",
        Position::RightMidfielder => "RightMidfielder",
        Position::LeftMidfielder => "LeftMidfielder",
        Position::RightWinger => "RightWinger",
        Position::LeftWinger => "LeftWinger",
        Position::Striker => "Striker",
    }
}

/// The stable code of a position's group: `goalkeeper`, `defender`, `midfielder`, `forward`.
pub fn position_group_code(group: &Position) -> &'static str {
    match group.to_group_position() {
        Position::Goalkeeper => "goalkeeper",
        Position::Defender => "defender",
        Position::Midfielder => "midfielder",
        Position::Forward => "forward",
        // `to_group_position` maps every variant into one of the four above; the arm keeps the
        // match exhaustive and unreachable.
        _ => "unknown",
    }
}

/// Fold a name to the lowercase, diacritic-free form both the importer and a search use.
///
/// SQLite's `unicode61` tokenizer removes combining marks but not precomposed letters such as
/// `ş` or `ø`, so a Turkish or Nordic name would only be findable by typing its accents. Folding
/// here means "sukru" finds "Şükrü" and "malmo" finds "Malmö", in both the index and the query.
///
/// Keeps ASCII letters and digits, maps the Latin letters SQLite does not fold, turns everything
/// else into a space, and collapses runs of spaces.
pub fn fold_name(name: &str) -> String {
    let mut folded = String::with_capacity(name.len());
    let mut pending_space = false;
    for lower in name.chars().flat_map(char::to_lowercase) {
        // Combining marks from a decomposed source ("S\u{0327}") carry no letter of their own.
        if ('\u{0300}'..='\u{036f}').contains(&lower) {
            continue;
        }
        let mapped: &str = match lower {
            'a'..='z' | '0'..='9' => {
                if pending_space && !folded.is_empty() {
                    folded.push(' ');
                }
                pending_space = false;
                folded.push(lower);
                continue;
            }
            'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' => "a",
            'ç' | 'ć' | 'č' | 'ĉ' | 'ċ' => "c",
            'ď' | 'đ' | 'ð' => "d",
            'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ĕ' | 'ė' | 'ę' | 'ě' => "e",
            'ĝ' | 'ğ' | 'ġ' | 'ģ' => "g",
            'ĥ' | 'ħ' => "h",
            'ì' | 'í' | 'î' | 'ï' | 'ĩ' | 'ī' | 'ĭ' | 'į' | 'ı' => "i",
            'ĵ' => "j",
            'ķ' => "k",
            'ĺ' | 'ļ' | 'ľ' | 'ł' => "l",
            'ñ' | 'ń' | 'ņ' | 'ň' => "n",
            'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ŏ' | 'ő' => "o",
            'ŕ' | 'ŗ' | 'ř' => "r",
            'ś' | 'š' | 'ş' | 'ŝ' | 'ș' => "s",
            'ß' => "ss",
            'ţ' | 'ť' | 'ŧ' => "t",
            'ù' | 'ú' | 'û' | 'ü' | 'ũ' | 'ū' | 'ŭ' | 'ů' | 'ű' | 'ų' => "u",
            'ŵ' => "w",
            'ý' | 'ÿ' | 'ŷ' => "y",
            'ź' | 'ż' | 'ž' => "z",
            'æ' => "ae",
            'œ' => "oe",
            'þ' => "th",
            _ => {
                pending_space = true;
                continue;
            }
        };
        if pending_space && !folded.is_empty() {
            folded.push(' ');
        }
        pending_space = false;
        folded.push_str(mapped);
    }
    folded
}

/// Turn free text into an FTS5 prefix expression over folded tokens, or `None` when it holds no
/// letter or digit.
///
/// Every token gets a `*` so a half-typed name still finds its player, and every token is a plain
/// lowercase word, so no FTS5 operator (`-`, `"`, `:`, `NEAR`) can leak in from user input and
/// turn a search into a syntax error.
pub fn fts_query(text: &str) -> Option<String> {
    let folded = fold_name(text);
    let tokens: Vec<&str> = folded.split_whitespace().collect();
    (!tokens.is_empty()).then(|| {
        tokens
            .iter()
            .map(|token| format!("{token}*"))
            .collect::<Vec<_>>()
            .join(" ")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folding_a_name_removes_case_and_diacritics() {
        assert_eq!(fold_name("Şükrü Yılmaz"), "sukru yilmaz");
        assert_eq!(fold_name("Malmö FF"), "malmo ff");
        assert_eq!(fold_name("Ødegaard"), "odegaard");
        assert_eq!(fold_name("N'Golo Kanté"), "n golo kante");
        assert_eq!(fold_name("S\u{0327}ükrü"), "sukru");
        assert_eq!(fold_name("  İlkay   Gündoğan  "), "ilkay gundogan");
        assert_eq!(fold_name(""), "");
        assert_eq!(fold_name("!!!"), "");
    }

    #[test]
    fn an_fts_query_is_plain_prefix_tokens_with_no_operator_left_in() {
        assert_eq!(fts_query("van der Sar"), Some("van* der* sar*".into()));
        assert_eq!(fts_query("Şükrü"), Some("sukru*".into()));
        assert_eq!(fts_query("a-b:c\"d"), Some("a* b* c* d*".into()));
        assert_eq!(fts_query("   "), None);
        assert_eq!(fts_query("!!!"), None);
    }

    #[test]
    fn a_page_is_clamped_to_a_renderable_window() {
        assert_eq!(Page::new(0, 0).limit, 1);
        assert_eq!(Page::new(0, 10_000).limit, MAX_PAGE_SIZE);
        assert_eq!(Page::default().limit, DEFAULT_PAGE_SIZE);
        assert_eq!(clamp_page_size(0), 1);
        assert_eq!(clamp_page_size(10_000), MAX_PAGE_SIZE);
        assert_eq!(clamp_page_size(25), 25);
    }

    fn summary(id: &str, full_name: &str, overall: Option<u8>) -> PlayerSummary {
        PlayerSummary {
            id: id.to_string(),
            full_name: full_name.to_string(),
            position: String::from("Striker"),
            position_group: String::from("forward"),
            nationality: String::from("ENG"),
            birth_date: None,
            birth_year: None,
            current_club_id: None,
            current_club_name: None,
            overall,
            potential: None,
        }
    }

    /// Given a page row and one of the four sort columns
    /// When its keyset cursor is taken
    /// Then the cursor carries that column's value and the order it belongs to.
    #[test]
    fn a_keyset_cursor_reads_the_column_its_sort_orders_by() {
        let mut player = summary("p-1", "Şükrü Yılmaz", Some(93));
        player.birth_year = Some(1998);
        player.potential = Some(97);

        let by_overall =
            PlayerKeysetCursor::after(PlayerSort::descending(PlayerSortColumn::Overall), &player);
        assert_eq!(
            by_overall.sort_value,
            PlayerSortValue::Number(Some(93)),
            "the cursor reads overall, not another rating"
        );
        assert_eq!(by_overall.player_id, "p-1");

        let by_potential =
            PlayerKeysetCursor::after(PlayerSort::ascending(PlayerSortColumn::Potential), &player);
        assert_eq!(by_potential.sort_value, PlayerSortValue::Number(Some(97)));

        let by_birth_year =
            PlayerKeysetCursor::after(PlayerSort::ascending(PlayerSortColumn::BirthYear), &player);
        assert_eq!(
            by_birth_year.sort_value,
            PlayerSortValue::Number(Some(1998))
        );

        let by_name =
            PlayerKeysetCursor::after(PlayerSort::descending(PlayerSortColumn::Name), &player);
        assert_eq!(
            by_name.sort_value,
            PlayerSortValue::Text(String::from("sukru yilmaz")),
            "a name cursor compares against the stored folded form"
        );
        assert_eq!(by_name.sort, PlayerSort::descending(PlayerSortColumn::Name));
    }

    /// Given a player whose rating and birth year the source never filled in
    /// When a cursor is taken in each numeric order
    /// Then the cursor records a NULL position rather than a zero.
    #[test]
    fn an_unrated_player_has_a_null_keyset_position_not_a_zero() {
        let unrated = summary("p-2", "Unknown", None);

        let by_overall =
            PlayerKeysetCursor::after(PlayerSort::descending(PlayerSortColumn::Overall), &unrated);
        let by_birth_year =
            PlayerKeysetCursor::after(PlayerSort::ascending(PlayerSortColumn::BirthYear), &unrated);

        assert_eq!(by_overall.sort_value, PlayerSortValue::Number(None));
        assert_eq!(by_birth_year.sort_value, PlayerSortValue::Number(None));
    }

    #[test]
    fn a_group_position_filter_maps_a_specific_position_to_its_group() {
        let striker = PositionFilter::group(Position::Striker);
        assert_eq!(striker.code(), "Striker");
        assert_eq!(striker.group_code(), Some("forward"));

        let centre_back = PositionFilter::exact(Position::CenterBack);
        assert_eq!(centre_back.code(), "CenterBack");
        assert_eq!(centre_back.group_code(), None);
    }

    #[test]
    fn every_position_has_a_distinct_code_and_a_known_group() {
        let mut codes = std::collections::BTreeSet::new();
        for position in Position::ALL {
            assert!(
                codes.insert(position_code(&position)),
                "{position:?} reused a code"
            );
            assert!(matches!(
                position_group_code(&position.to_group_position()),
                "goalkeeper" | "defender" | "midfielder" | "forward"
            ));
        }
        assert_eq!(codes.len(), Position::ALL.len());
    }
}
