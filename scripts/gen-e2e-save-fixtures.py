#!/usr/bin/env python3
"""Generate checked-in old-schema save fixtures for hoshi-tui e2e tests.

Reads SCHEMA_VERSION from crates/hoshi-core/src/save.rs and writes one sqlite
file per promised migration (v1 through current-1), plus a newer-than-supported
save and a deterministic expedition fixture.

Run from the repository root:

    python3 scripts/gen-e2e-save-fixtures.py
"""

from __future__ import annotations

import re
import sqlite3
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SAVE_RS = ROOT / "crates" / "hoshi-core" / "src" / "save.rs"
OUT_DIR = ROOT / "crates" / "hoshi-tui" / "tests" / "e2e" / "fixtures"

# Far enough in the future that a live `state` command is a catch-up no-op.
FUTURE_TICK = 4_102_444_800


def current_schema() -> int:
    text = SAVE_RS.read_text(encoding="utf-8")
    match = re.search(r"pub const SCHEMA_VERSION: u32 = (\d+);", text)
    if not match:
        raise SystemExit("SCHEMA_VERSION not found in save.rs")
    return int(match.group(1))


def table_exists(conn: sqlite3.Connection, table: str) -> bool:
    row = conn.execute(
        "SELECT 1 FROM sqlite_master WHERE type='table' AND name=?",
        (table,),
    ).fetchone()
    return row is not None


def has_column(conn: sqlite3.Connection, table: str, column: str) -> bool:
    rows = conn.execute(f"PRAGMA table_info({table})").fetchall()
    return any(row[1] == column for row in rows)


def add_column(conn: sqlite3.Connection, table: str, column: str, sql: str) -> None:
    if not has_column(conn, table, column):
        conn.execute(sql)


def migrate(conn: sqlite3.Connection, version: int) -> None:
    if version == 1:
        conn.executescript(
            """
            CREATE TABLE IF NOT EXISTS meta (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                last_tick_unix INTEGER NOT NULL,
                rng_seed INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS resources (
                planet_id INTEGER PRIMARY KEY,
                metal REAL NOT NULL,
                crystal REAL NOT NULL,
                deuterium REAL NOT NULL
            );
            CREATE TABLE IF NOT EXISTS buildings (
                planet_id INTEGER NOT NULL,
                kind TEXT NOT NULL,
                level INTEGER NOT NULL,
                PRIMARY KEY (planet_id, kind)
            );
            CREATE TABLE IF NOT EXISTS build_queue (
                planet_id INTEGER NOT NULL,
                position INTEGER NOT NULL,
                kind TEXT NOT NULL,
                from_level INTEGER NOT NULL,
                paid_metal REAL NOT NULL,
                paid_crystal REAL NOT NULL,
                paid_deuterium REAL NOT NULL,
                started_at INTEGER,
                completes_at INTEGER,
                PRIMARY KEY (planet_id, position)
            );
            CREATE TABLE IF NOT EXISTS tech_levels (
                id TEXT PRIMARY KEY,
                level INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS research (
                tech TEXT PRIMARY KEY,
                from_level INTEGER NOT NULL,
                paid_metal REAL NOT NULL,
                paid_crystal REAL NOT NULL,
                paid_deuterium REAL NOT NULL,
                started_at INTEGER NOT NULL,
                completes_at INTEGER NOT NULL
            );
            """
        )
    elif version == 2:
        conn.executescript(
            """
            CREATE TABLE IF NOT EXISTS ships (
                planet_id INTEGER NOT NULL,
                kind TEXT NOT NULL,
                count INTEGER NOT NULL,
                PRIMARY KEY (planet_id, kind)
            );
            CREATE TABLE IF NOT EXISTS ship_queue (
                planet_id INTEGER NOT NULL,
                position INTEGER NOT NULL,
                kind TEXT NOT NULL,
                remaining INTEGER NOT NULL,
                paid_metal REAL NOT NULL,
                paid_crystal REAL NOT NULL,
                paid_deuterium REAL NOT NULL,
                started_at INTEGER,
                completes_at INTEGER,
                PRIMARY KEY (planet_id, position)
            );
            """
        )
    elif version == 3:
        conn.executescript(
            """
            CREATE TABLE IF NOT EXISTS missions (
                id INTEGER PRIMARY KEY,
                origin_g INTEGER NOT NULL,
                origin_s INTEGER NOT NULL,
                origin_p INTEGER NOT NULL,
                dest_g INTEGER NOT NULL,
                dest_s INTEGER NOT NULL,
                dest_p INTEGER NOT NULL,
                speed_percent INTEGER NOT NULL,
                phase TEXT NOT NULL,
                launched_at INTEGER NOT NULL,
                outbound_secs INTEGER NOT NULL,
                hold_secs INTEGER NOT NULL,
                phase_ends_at INTEGER NOT NULL,
                fuel_paid REAL NOT NULL
            );
            CREATE TABLE IF NOT EXISTS mission_ships (
                mission_id INTEGER NOT NULL,
                kind TEXT NOT NULL,
                count INTEGER NOT NULL,
                PRIMARY KEY (mission_id, kind)
            );
            """
        )
    elif version == 4:
        conn.executescript(
            """
            CREATE TABLE IF NOT EXISTS raids (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                next_raid_at INTEGER
            );
            CREATE TABLE IF NOT EXISTS event_log (
                position INTEGER PRIMARY KEY,
                at INTEGER NOT NULL,
                text TEXT NOT NULL
            );
            """
        )
    elif version == 5:
        add_column(
            conn, "meta", "galaxy_seed",
            "ALTER TABLE meta ADD COLUMN galaxy_seed INTEGER NOT NULL DEFAULT 0",
        )
        add_column(
            conn, "meta", "home_kind",
            "ALTER TABLE meta ADD COLUMN home_kind TEXT NOT NULL DEFAULT 'rocky'",
        )
        add_column(
            conn, "meta", "home_size",
            "ALTER TABLE meta ADD COLUMN home_size INTEGER NOT NULL DEFAULT 163",
        )
        add_column(
            conn, "missions", "colonize",
            "ALTER TABLE missions ADD COLUMN colonize INTEGER NOT NULL DEFAULT 0",
        )
        conn.executescript(
            """
            CREATE TABLE IF NOT EXISTS colonies (
                planet_id INTEGER PRIMARY KEY,
                galaxy INTEGER NOT NULL,
                system INTEGER NOT NULL,
                position INTEGER NOT NULL,
                kind TEXT NOT NULL,
                size INTEGER NOT NULL
            );
            """
        )
    elif version == 6:
        conn.executescript(
            """
            CREATE TABLE IF NOT EXISTS activity_boosts (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                prod_until INTEGER,
                prod_factor REAL NOT NULL DEFAULT 0,
                research_until INTEGER,
                research_factor REAL NOT NULL DEFAULT 0
            );
            CREATE TABLE IF NOT EXISTS activity_limits (
                channel TEXT PRIMARY KEY,
                last_applied INTEGER,
                day_start INTEGER NOT NULL DEFAULT 0,
                day_count INTEGER NOT NULL DEFAULT 0
            );
            """
        )
    elif version == 7:
        add_column(
            conn, "missions", "expedition_seed",
            "ALTER TABLE missions ADD COLUMN expedition_seed INTEGER",
        )
        add_column(
            conn, "missions", "expedition_applied",
            "ALTER TABLE missions ADD COLUMN expedition_applied INTEGER NOT NULL DEFAULT 0",
        )
    elif version == 8:
        for column, sql in (
            ("cargo_metal", "ALTER TABLE missions ADD COLUMN cargo_metal REAL NOT NULL DEFAULT 0"),
            ("cargo_crystal", "ALTER TABLE missions ADD COLUMN cargo_crystal REAL NOT NULL DEFAULT 0"),
            ("cargo_deuterium", "ALTER TABLE missions ADD COLUMN cargo_deuterium REAL NOT NULL DEFAULT 0"),
            ("transport", "ALTER TABLE missions ADD COLUMN transport INTEGER NOT NULL DEFAULT 0"),
        ):
            add_column(conn, "missions", column, sql)
        conn.executescript(
            """
            CREATE TABLE IF NOT EXISTS routes (
                id INTEGER PRIMARY KEY,
                origin_g INTEGER NOT NULL,
                origin_s INTEGER NOT NULL,
                origin_p INTEGER NOT NULL,
                dest_g INTEGER NOT NULL,
                dest_s INTEGER NOT NULL,
                dest_p INTEGER NOT NULL,
                cargo_metal REAL NOT NULL,
                cargo_crystal REAL NOT NULL,
                cargo_deuterium REAL NOT NULL,
                speed_percent INTEGER NOT NULL,
                interval_secs INTEGER NOT NULL,
                next_at INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS route_ships (
                route_id INTEGER NOT NULL,
                kind TEXT NOT NULL,
                count INTEGER NOT NULL,
                PRIMARY KEY (route_id, kind)
            );
            """
        )
    elif version == 9:
        add_column(
            conn, "meta", "deterministic",
            "ALTER TABLE meta ADD COLUMN deterministic INTEGER NOT NULL DEFAULT 0",
        )
    elif version == 10:
        conn.executescript(
            """
            CREATE TABLE IF NOT EXISTS wonder_job (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                wonder_id TEXT NOT NULL,
                stage INTEGER NOT NULL,
                metal REAL NOT NULL,
                crystal REAL NOT NULL,
                deuterium REAL NOT NULL,
                completes_at INTEGER
            );
            CREATE TABLE IF NOT EXISTS wonder_done (
                position INTEGER PRIMARY KEY,
                wonder_id TEXT NOT NULL
            );
            """
        )
    elif version == 11:
        add_column(
            conn, "meta", "outposts_seeded",
            "ALTER TABLE meta ADD COLUMN outposts_seeded INTEGER NOT NULL DEFAULT 0",
        )
        add_column(conn, "colonies", "rare", "ALTER TABLE colonies ADD COLUMN rare TEXT")
        add_column(
            conn, "missions", "attack",
            "ALTER TABLE missions ADD COLUMN attack INTEGER NOT NULL DEFAULT 0",
        )
        add_column(
            conn, "missions", "invade",
            "ALTER TABLE missions ADD COLUMN invade INTEGER NOT NULL DEFAULT 0",
        )
        conn.executescript(
            """
            CREATE TABLE IF NOT EXISTS outposts (
                planet_id INTEGER PRIMARY KEY,
                galaxy INTEGER NOT NULL,
                system INTEGER NOT NULL,
                position INTEGER NOT NULL,
                kind TEXT NOT NULL,
                size INTEGER NOT NULL,
                garrison INTEGER NOT NULL,
                rare TEXT,
                last_rebuild_at INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS outpost_ships (
                planet_id INTEGER NOT NULL,
                kind TEXT NOT NULL,
                count INTEGER NOT NULL,
                PRIMARY KEY (planet_id, kind)
            );
            """
        )
    elif version == 12:
        conn.executescript(
            """
            CREATE TABLE IF NOT EXISTS prestige (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                currency INTEGER NOT NULL,
                lifetime INTEGER NOT NULL,
                run_index INTEGER NOT NULL,
                run_started_at INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS prestige_owned (
                bonus_id TEXT PRIMARY KEY,
                count INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS prestige_runs (
                position INTEGER PRIMARY KEY,
                run_index INTEGER NOT NULL,
                started_at INTEGER NOT NULL,
                ended_at INTEGER,
                score INTEGER NOT NULL,
                prestige_earned INTEGER NOT NULL,
                wonders INTEGER NOT NULL,
                colonies INTEGER NOT NULL,
                building_levels INTEGER NOT NULL,
                research_levels INTEGER NOT NULL
            );
            """
        )
    elif version == 13:
        conn.executescript(
            """
            CREATE TABLE IF NOT EXISTS goods (
                planet_id INTEGER NOT NULL,
                good TEXT NOT NULL,
                amount REAL NOT NULL,
                PRIMARY KEY (planet_id, good)
            );
            """
        )
    elif version == 14:
        add_column(
            conn, "meta", "generator_version",
            "ALTER TABLE meta ADD COLUMN generator_version INTEGER NOT NULL DEFAULT 1",
        )
    elif version == 15:
        add_column(
            conn, "meta", "last_story_at",
            "ALTER TABLE meta ADD COLUMN last_story_at INTEGER NOT NULL DEFAULT 0",
        )
        conn.executescript(
            """
            CREATE TABLE IF NOT EXISTS story_pending (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                event_id TEXT NOT NULL,
                offered_at INTEGER NOT NULL,
                timeout_at INTEGER NOT NULL
            );
            """
        )
    elif version == 16:
        add_column(
            conn, "meta", "last_market_at",
            "ALTER TABLE meta ADD COLUMN last_market_at INTEGER NOT NULL DEFAULT 0",
        )
        conn.executescript(
            """
            CREATE TABLE IF NOT EXISTS market_prices (
                kind TEXT PRIMARY KEY,
                price REAL NOT NULL
            );
            CREATE TABLE IF NOT EXISTS market_history (
                kind TEXT NOT NULL,
                position INTEGER NOT NULL,
                price REAL NOT NULL,
                PRIMARY KEY (kind, position)
            );
            """
        )
    elif version == 17:
        add_column(
            conn, "meta", "last_diplomacy_at",
            "ALTER TABLE meta ADD COLUMN last_diplomacy_at INTEGER NOT NULL DEFAULT 0",
        )
        conn.executescript(
            """
            CREATE TABLE IF NOT EXISTS faction_standing (
                id TEXT PRIMARY KEY,
                standing REAL NOT NULL
            );
            CREATE TABLE IF NOT EXISTS faction_log (
                position INTEGER PRIMARY KEY,
                at INTEGER NOT NULL,
                faction TEXT NOT NULL,
                delta REAL NOT NULL,
                reason TEXT NOT NULL
            );
            """
        )
    elif version == 18:
        add_column(
            conn, "missions", "spy",
            "ALTER TABLE missions ADD COLUMN spy INTEGER NOT NULL DEFAULT 0",
        )
        conn.executescript(
            """
            CREATE TABLE IF NOT EXISTS spy_reports (
                id INTEGER PRIMARY KEY,
                galaxy INTEGER NOT NULL,
                system INTEGER NOT NULL,
                position INTEGER NOT NULL,
                created_at INTEGER NOT NULL,
                probes_sent INTEGER NOT NULL,
                probes_lost INTEGER NOT NULL,
                detected INTEGER NOT NULL,
                score REAL NOT NULL,
                tier TEXT NOT NULL,
                metal REAL NOT NULL DEFAULT 0,
                crystal REAL NOT NULL DEFAULT 0,
                deuterium REAL NOT NULL DEFAULT 0,
                garrison INTEGER NOT NULL DEFAULT 0,
                kind TEXT,
                size INTEGER,
                revealed_resources INTEGER NOT NULL DEFAULT 0,
                revealed_fleet INTEGER NOT NULL DEFAULT 0,
                revealed_defences INTEGER NOT NULL DEFAULT 0,
                revealed_garrison INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE IF NOT EXISTS spy_report_ships (
                report_id INTEGER NOT NULL,
                role TEXT NOT NULL,
                kind TEXT NOT NULL,
                count INTEGER NOT NULL,
                PRIMARY KEY (report_id, role, kind)
            );
            """
        )
    elif version == 19:
        conn.executescript(
            """
            CREATE TABLE IF NOT EXISTS planet_defences (
                planet_id INTEGER NOT NULL,
                kind TEXT NOT NULL,
                count INTEGER NOT NULL,
                PRIMARY KEY (planet_id, kind)
            );
            """
        )
    elif version == 20:
        conn.executescript(
            """
            CREATE TABLE IF NOT EXISTS laws (
                id INTEGER PRIMARY KEY,
                template_id TEXT NOT NULL,
                param REAL NOT NULL,
                previous_param REAL,
                owner TEXT NOT NULL,
                jurisdiction TEXT NOT NULL,
                proposed_at INTEGER NOT NULL,
                effective_at INTEGER NOT NULL,
                phase_in_secs INTEGER NOT NULL,
                status TEXT NOT NULL,
                repeal_effective_at INTEGER,
                enacted_at INTEGER,
                repealed_at INTEGER,
                baseline_output REAL NOT NULL DEFAULT 0,
                baseline_pollution REAL NOT NULL DEFAULT 0,
                baseline_research REAL NOT NULL DEFAULT 0,
                last_sample_at INTEGER NOT NULL DEFAULT 0,
                revenue_accrued REAL NOT NULL DEFAULT 0
            );
            CREATE TABLE IF NOT EXISTS law_approvals (
                law_id INTEGER NOT NULL,
                who TEXT NOT NULL,
                at INTEGER NOT NULL,
                PRIMARY KEY (law_id, who)
            );
            CREATE TABLE IF NOT EXISTS law_actions (
                position INTEGER PRIMARY KEY,
                at INTEGER NOT NULL,
                kind TEXT NOT NULL,
                law_id INTEGER NOT NULL,
                actor TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS law_samples (
                law_id INTEGER NOT NULL,
                at INTEGER NOT NULL,
                metric TEXT NOT NULL,
                expected REAL NOT NULL,
                actual REAL NOT NULL,
                PRIMARY KEY (law_id, at, metric)
            );
            """
        )
    elif version == 21:
        conn.executescript(
            """
            CREATE TABLE IF NOT EXISTS exclusive_picks (
                group_id TEXT PRIMARY KEY,
                tech_id TEXT NOT NULL
            );
            """
        )
    elif version == 22:
        conn.executescript(
            """
            CREATE TABLE IF NOT EXISTS achievements (
                id TEXT PRIMARY KEY,
                unlocked_at INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS achievement_stats (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                raids_survived INTEGER NOT NULL DEFAULT 0,
                outposts_conquered INTEGER NOT NULL DEFAULT 0
            );
            """
        )
    elif version == 23:
        conn.executescript(
            """
            CREATE TABLE IF NOT EXISTS combat_logs (
                id INTEGER PRIMARY KEY,
                at INTEGER NOT NULL,
                seed INTEGER NOT NULL,
                winner TEXT NOT NULL,
                rounds INTEGER NOT NULL,
                layered INTEGER NOT NULL DEFAULT 0,
                ground_atk INTEGER,
                ground_def INTEGER,
                ground_atk_roll INTEGER,
                ground_def_roll INTEGER,
                ground_winner TEXT
            );
            CREATE TABLE IF NOT EXISTS combat_log_frames (
                log_id INTEGER NOT NULL,
                frame INTEGER NOT NULL,
                round INTEGER NOT NULL,
                shots INTEGER NOT NULL,
                explosions INTEGER NOT NULL,
                PRIMARY KEY (log_id, frame)
            );
            CREATE TABLE IF NOT EXISTS combat_log_ships (
                log_id INTEGER NOT NULL,
                frame INTEGER NOT NULL,
                side TEXT NOT NULL,
                kind TEXT NOT NULL,
                count INTEGER NOT NULL,
                shots INTEGER NOT NULL DEFAULT 0,
                explosions INTEGER NOT NULL DEFAULT 0
            );
            """
        )
    elif version == 24:
        conn.executescript(
            """
            CREATE TABLE IF NOT EXISTS daily_meta (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                day_index INTEGER,
                streak INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE IF NOT EXISTS daily_slots (
                ord INTEGER PRIMARY KEY,
                spec_id TEXT NOT NULL,
                progress INTEGER NOT NULL,
                target INTEGER NOT NULL,
                rewarded INTEGER NOT NULL
            );
            """
        )
    elif version == 25:
        conn.executescript(
            """
            CREATE TABLE IF NOT EXISTS moons (
                galaxy INTEGER NOT NULL,
                system INTEGER NOT NULL,
                position INTEGER NOT NULL,
                phalanx INTEGER NOT NULL,
                jump_gate INTEGER NOT NULL,
                jump_ready_at INTEGER NOT NULL,
                PRIMARY KEY (galaxy, system, position)
            );
            CREATE TABLE IF NOT EXISTS debris (
                galaxy INTEGER NOT NULL,
                system INTEGER NOT NULL,
                position INTEGER NOT NULL,
                amount REAL NOT NULL,
                PRIMARY KEY (galaxy, system, position)
            );
            """
        )
    elif version == 26:
        conn.executescript(
            """
            CREATE TABLE IF NOT EXISTS tutorial_meta (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                enabled INTEGER NOT NULL DEFAULT 0,
                skipped INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE IF NOT EXISTS tutorial_seen (
                hint_id TEXT PRIMARY KEY
            );
            """
        )
    elif version == 27:
        conn.executescript(
            """
            CREATE TABLE IF NOT EXISTS fleet_tactics (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                formation TEXT NOT NULL,
                stance TEXT NOT NULL,
                retreat_below REAL NOT NULL,
                home_admiral_id INTEGER,
                next_admiral_id INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS admirals (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                trait_id TEXT NOT NULL,
                xp INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS reserve_ships (
                kind TEXT PRIMARY KEY,
                count INTEGER NOT NULL
            );
            """
        )
        if table_exists(conn, "spy_reports"):
            for column in ("formation", "stance"):
                if not has_column(conn, "spy_reports", column):
                    conn.execute(f"ALTER TABLE spy_reports ADD COLUMN {column} TEXT")
    elif version == 28:
        conn.executescript(
            """
            CREATE TABLE IF NOT EXISTS history_events (
                id INTEGER PRIMARY KEY,
                at INTEGER NOT NULL,
                kind TEXT NOT NULL,
                actor TEXT NOT NULL,
                action TEXT NOT NULL,
                objects TEXT NOT NULL,
                inputs TEXT NOT NULL,
                outcome TEXT NOT NULL,
                reason TEXT NOT NULL,
                cause_key TEXT,
                parent_ids TEXT NOT NULL,
                energy REAL NOT NULL,
                heat REAL NOT NULL,
                mass REAL NOT NULL
            );
            CREATE TABLE IF NOT EXISTS history_snapshots (
                at INTEGER NOT NULL,
                event_id INTEGER NOT NULL,
                metal REAL NOT NULL,
                crystal REAL NOT NULL,
                deuterium REAL NOT NULL,
                energy REAL NOT NULL,
                heat REAL NOT NULL,
                home_kind TEXT NOT NULL,
                building_levels INTEGER NOT NULL,
                PRIMARY KEY (event_id)
            );
            """
        )
    elif version == 29:
        conn.executescript(
            """
            CREATE TABLE IF NOT EXISTS leaders (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                species TEXT NOT NULL,
                trait_id TEXT NOT NULL,
                skill_command INTEGER NOT NULL,
                skill_logistics INTEGER NOT NULL,
                skill_policy INTEGER NOT NULL,
                xp INTEGER NOT NULL,
                born_at INTEGER NOT NULL,
                lifespan_secs INTEGER NOT NULL,
                retired INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS assignments (
                leader_id INTEGER PRIMARY KEY,
                role TEXT NOT NULL,
                target TEXT NOT NULL,
                assigned_at INTEGER NOT NULL,
                ready_at INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS delegations (
                leader_id INTEGER NOT NULL,
                permission TEXT NOT NULL,
                spend_cap REAL,
                spent REAL NOT NULL,
                period_start INTEGER NOT NULL,
                PRIMARY KEY (leader_id, permission)
            );
            """
        )
    elif version == 30:
        conn.executescript(
            """
            CREATE TABLE IF NOT EXISTS vehicles (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                galaxy INTEGER NOT NULL,
                system INTEGER NOT NULL,
                position INTEGER NOT NULL,
                free_at INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS vehicle_ships (
                vehicle_id INTEGER NOT NULL,
                kind TEXT NOT NULL,
                count INTEGER NOT NULL,
                PRIMARY KEY (vehicle_id, kind)
            );
            CREATE TABLE IF NOT EXISTS planned_missions (
                id INTEGER PRIMARY KEY,
                vehicle_id INTEGER NOT NULL,
                origin_g INTEGER NOT NULL,
                origin_s INTEGER NOT NULL,
                origin_p INTEGER NOT NULL,
                dest_g INTEGER NOT NULL,
                dest_s INTEGER NOT NULL,
                dest_p INTEGER NOT NULL,
                transfer TEXT NOT NULL,
                customer TEXT NOT NULL,
                window_opens INTEGER NOT NULL,
                window_closes INTEGER NOT NULL,
                launched_at INTEGER NOT NULL,
                arrives_at INTEGER NOT NULL,
                delta_v REAL NOT NULL,
                fuel_mass REAL NOT NULL,
                cargo_metal REAL NOT NULL,
                cargo_crystal REAL NOT NULL,
                cargo_deuterium REAL NOT NULL
            );
            """
        )
    elif version == 31:
        conn.executescript(
            """
            CREATE TABLE IF NOT EXISTS classified (
                kind TEXT NOT NULL,
                object_id TEXT NOT NULL,
                classification TEXT NOT NULL,
                compartments TEXT NOT NULL,
                PRIMARY KEY (kind, object_id)
            );
            CREATE TABLE IF NOT EXISTS sharing_agreements (
                faction TEXT PRIMARY KEY,
                classification TEXT NOT NULL,
                compartments TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS leak_incidents (
                id INTEGER PRIMARY KEY,
                at INTEGER NOT NULL,
                source TEXT NOT NULL,
                kind TEXT NOT NULL,
                object_id TEXT NOT NULL,
                via TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS leader_clearance (
                leader_id INTEGER PRIMARY KEY,
                classification TEXT NOT NULL,
                compartments TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS classification_meta (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                next_incident_id INTEGER NOT NULL,
                next_disloyal_at INTEGER NOT NULL
            );
            """
        )
    elif version == 32:
        conn.executescript(
            """
            CREATE TABLE IF NOT EXISTS market_regions (
                galaxy INTEGER NOT NULL,
                system INTEGER NOT NULL,
                position INTEGER NOT NULL,
                npc_metal REAL NOT NULL,
                npc_crystal REAL NOT NULL,
                npc_deuterium REAL NOT NULL,
                player_metal REAL NOT NULL,
                player_crystal REAL NOT NULL,
                player_deuterium REAL NOT NULL,
                PRIMARY KEY (galaxy, system, position)
            );
            CREATE TABLE IF NOT EXISTS market_region_history (
                galaxy INTEGER NOT NULL,
                system INTEGER NOT NULL,
                position INTEGER NOT NULL,
                kind TEXT NOT NULL,
                hist_index INTEGER NOT NULL,
                price REAL NOT NULL,
                PRIMARY KEY (galaxy, system, position, kind, hist_index)
            );
            CREATE TABLE IF NOT EXISTS market_orders (
                id INTEGER PRIMARY KEY,
                galaxy INTEGER NOT NULL,
                system INTEGER NOT NULL,
                position INTEGER NOT NULL,
                trader TEXT NOT NULL,
                side TEXT NOT NULL,
                good TEXT NOT NULL,
                remaining REAL NOT NULL,
                price REAL NOT NULL,
                escrow_metal REAL NOT NULL,
                escrow_good REAL NOT NULL,
                created_at INTEGER NOT NULL,
                expires_at INTEGER,
                is_market INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS market_contracts (
                id INTEGER PRIMARY KEY,
                kind TEXT NOT NULL,
                galaxy INTEGER NOT NULL,
                system INTEGER NOT NULL,
                position INTEGER NOT NULL,
                good TEXT NOT NULL,
                amount REAL NOT NULL,
                remaining REAL NOT NULL,
                price REAL NOT NULL,
                deadline INTEGER NOT NULL,
                penalty REAL NOT NULL,
                issuer TEXT NOT NULL,
                holder TEXT,
                status TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS market_shipments (
                id INTEGER PRIMARY KEY,
                origin_g INTEGER NOT NULL,
                origin_s INTEGER NOT NULL,
                origin_p INTEGER NOT NULL,
                dest_g INTEGER NOT NULL,
                dest_s INTEGER NOT NULL,
                dest_p INTEGER NOT NULL,
                metal REAL NOT NULL,
                crystal REAL NOT NULL,
                deuterium REAL NOT NULL,
                fuel REAL NOT NULL,
                launched_at INTEGER NOT NULL,
                arrives_at INTEGER NOT NULL,
                contract_id INTEGER,
                trader TEXT NOT NULL,
                delivered INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS market_board_meta (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                next_order_id INTEGER NOT NULL,
                next_contract_id INTEGER NOT NULL,
                next_shipment_id INTEGER NOT NULL
            );
            """
        )
    elif version == 33:
        if not has_column(conn, "outposts", "homeworld"):
            conn.execute(
                "ALTER TABLE outposts ADD COLUMN homeworld INTEGER NOT NULL DEFAULT 0"
            )
        conn.executescript(
            """
            CREATE TABLE IF NOT EXISTS victory (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                path TEXT,
                at INTEGER,
                prestige INTEGER,
                elapsed_secs INTEGER,
                kept_playing INTEGER,
                allied_since INTEGER,
                pirate_g INTEGER NOT NULL DEFAULT 1,
                pirate_s INTEGER NOT NULL DEFAULT 80,
                pirate_p INTEGER NOT NULL DEFAULT 16
            );
            """
        )
    elif version == 34:
        conn.executescript(
            """
            CREATE TABLE IF NOT EXISTS season (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                ruleset_id TEXT NOT NULL,
                window_start INTEGER NOT NULL,
                window_end INTEGER NOT NULL,
                seed INTEGER NOT NULL,
                prestige INTEGER NOT NULL
            );
            """
        )
    elif version == 35:
        conn.executescript(
            """
            CREATE TABLE IF NOT EXISTS species_run (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                species_id TEXT NOT NULL,
                brew_until INTEGER,
                flag_g INTEGER,
                flag_s INTEGER,
                flag_p INTEGER,
                flag_arrives INTEGER,
                flag_anchored INTEGER
            );
            CREATE TABLE IF NOT EXISTS prestige_species (
                species_id TEXT PRIMARY KEY
            );
            """
        )
    elif version == 36:
        conn.executescript(
            """
            CREATE TABLE IF NOT EXISTS rivals_meta (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                enabled INTEGER NOT NULL,
                tick_secs INTEGER NOT NULL,
                aggression REAL NOT NULL,
                efficiency REAL NOT NULL,
                starting_bonus REAL NOT NULL
            );
            CREATE TABLE IF NOT EXISTS rivals (
                rival_id TEXT PRIMARY KEY,
                personality TEXT NOT NULL,
                galaxy INTEGER NOT NULL,
                system INTEGER NOT NULL,
                position INTEGER NOT NULL,
                metal REAL NOT NULL,
                crystal REAL NOT NULL,
                deuterium REAL NOT NULL,
                last_tick INTEGER NOT NULL,
                next_act_at INTEGER NOT NULL,
                last_action TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS rival_buildings (
                rival_id TEXT NOT NULL,
                kind TEXT NOT NULL,
                level INTEGER NOT NULL,
                PRIMARY KEY (rival_id, kind)
            );
            CREATE TABLE IF NOT EXISTS rival_queue (
                rival_id TEXT NOT NULL,
                position INTEGER NOT NULL,
                kind TEXT NOT NULL,
                from_level INTEGER NOT NULL,
                paid_metal REAL NOT NULL,
                paid_crystal REAL NOT NULL,
                paid_deuterium REAL NOT NULL,
                started_at INTEGER,
                completes_at INTEGER,
                PRIMARY KEY (rival_id, position)
            );
            CREATE TABLE IF NOT EXISTS rival_ships (
                rival_id TEXT NOT NULL,
                kind TEXT NOT NULL,
                count INTEGER NOT NULL,
                PRIMARY KEY (rival_id, kind)
            );
            CREATE TABLE IF NOT EXISTS rival_techs (
                rival_id TEXT NOT NULL,
                tech_id TEXT NOT NULL,
                level INTEGER NOT NULL,
                PRIMARY KEY (rival_id, tech_id)
            );
            CREATE TABLE IF NOT EXISTS rival_research (
                rival_id TEXT PRIMARY KEY,
                tech TEXT NOT NULL,
                from_level INTEGER NOT NULL,
                paid_metal REAL NOT NULL,
                paid_crystal REAL NOT NULL,
                paid_deuterium REAL NOT NULL,
                started_at INTEGER NOT NULL,
                completes_at INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS rival_ship_queue (
                rival_id TEXT NOT NULL,
                position INTEGER NOT NULL,
                kind TEXT NOT NULL,
                remaining INTEGER NOT NULL,
                paid_metal REAL NOT NULL,
                paid_crystal REAL NOT NULL,
                paid_deuterium REAL NOT NULL,
                started_at INTEGER,
                completes_at INTEGER,
                PRIMARY KEY (rival_id, position)
            );
            """
        )
    elif version == 37:
        conn.executescript(
            """
            CREATE TABLE IF NOT EXISTS ladder (
                tier_id TEXT PRIMARY KEY,
                completed_at INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS ladder_hold (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                tier_id TEXT,
                since INTEGER
            );
            """
        )
    elif version == 38:
        conn.executescript(
            """
            CREATE TABLE IF NOT EXISTS frontier_meta (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                revealed INTEGER NOT NULL,
                research_open INTEGER NOT NULL,
                wonder_open INTEGER NOT NULL,
                bridge_sectors INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS frontier_seen (
                event_id TEXT PRIMARY KEY
            );
            CREATE TABLE IF NOT EXISTS frontier_worlds (
                galaxy INTEGER NOT NULL,
                system INTEGER NOT NULL,
                position INTEGER NOT NULL,
                sector INTEGER NOT NULL,
                kind TEXT NOT NULL,
                size INTEGER NOT NULL,
                rare TEXT,
                PRIMARY KEY (galaxy, system, position)
            );
            """
        )
    elif version == 39:
        conn.executescript(
            """
            CREATE TABLE IF NOT EXISTS automation_rules (
                id INTEGER PRIMARY KEY,
                enabled INTEGER NOT NULL,
                cond_kind TEXT NOT NULL,
                cond_g INTEGER NOT NULL,
                cond_s INTEGER NOT NULL,
                cond_p INTEGER NOT NULL,
                cond_resource TEXT NOT NULL,
                cond_amount REAL NOT NULL,
                act_kind TEXT NOT NULL,
                from_g INTEGER,
                from_s INTEGER,
                from_p INTEGER,
                to_g INTEGER,
                to_s INTEGER,
                to_p INTEGER,
                act_resource TEXT,
                act_amount REAL,
                act_g INTEGER,
                act_s INTEGER,
                act_p INTEGER,
                act_building TEXT
            );
            CREATE TABLE IF NOT EXISTS automation_meta (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                next_at INTEGER
            );
            """
        )
    elif version == 40:
        conn.executescript(
            """
            ALTER TABLE resources ADD COLUMN rare_earths REAL NOT NULL DEFAULT 0;
            ALTER TABLE resources ADD COLUMN water_ice REAL NOT NULL DEFAULT 0;
            ALTER TABLE resources ADD COLUMN helium_3 REAL NOT NULL DEFAULT 0;
            """
        )
        for table, column in (
            ("missions", "cargo_rare_earths"),
            ("missions", "cargo_water_ice"),
            ("missions", "cargo_helium_3"),
            ("routes", "cargo_rare_earths"),
            ("routes", "cargo_water_ice"),
            ("routes", "cargo_helium_3"),
            ("planned_missions", "cargo_rare_earths"),
            ("planned_missions", "cargo_water_ice"),
            ("planned_missions", "cargo_helium_3"),
            ("market_shipments", "rare_earths"),
            ("market_shipments", "water_ice"),
            ("market_shipments", "helium_3"),
            ("market_regions", "npc_rare_earths"),
            ("market_regions", "npc_water_ice"),
            ("market_regions", "npc_helium_3"),
            ("market_regions", "player_rare_earths"),
            ("market_regions", "player_water_ice"),
            ("market_regions", "player_helium_3"),
        ):
            add_column(
                conn,
                table,
                column,
                f"ALTER TABLE {table} ADD COLUMN {column} REAL NOT NULL DEFAULT 0",
            )
    elif version == 41:
        conn.executescript(
            """
            CREATE TABLE IF NOT EXISTS finance_meta (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                cash REAL NOT NULL,
                reserve_target REAL NOT NULL,
                period_started_at INTEGER NOT NULL,
                next_period_at INTEGER NOT NULL,
                next_spend_id INTEGER NOT NULL,
                inc_production REAL NOT NULL,
                inc_tax REAL NOT NULL,
                inc_trade REAL NOT NULL,
                inc_contract REAL NOT NULL,
                exp_industry REAL NOT NULL,
                exp_research REAL NOT NULL,
                exp_military REAL NOT NULL,
                exp_logistics REAL NOT NULL,
                exp_interest REAL NOT NULL
            );
            CREATE TABLE IF NOT EXISTS finance_budgets (
                center TEXT PRIMARY KEY,
                limit_val REAL,
                actual REAL NOT NULL,
                forecast REAL NOT NULL,
                last_actual REAL NOT NULL,
                last_forecast REAL NOT NULL
            );
            CREATE TABLE IF NOT EXISTS finance_pending (
                id INTEGER PRIMARY KEY,
                center TEXT NOT NULL,
                amount REAL NOT NULL,
                actor TEXT NOT NULL,
                kind TEXT NOT NULL,
                requested_at INTEGER NOT NULL,
                g INTEGER,
                s INTEGER,
                p INTEGER,
                building TEXT,
                tech TEXT,
                ship TEXT,
                defence TEXT,
                count INTEGER
            );
            CREATE TABLE IF NOT EXISTS finance_job_credits (
                id INTEGER PRIMARY KEY,
                scope TEXT NOT NULL,
                idx INTEGER NOT NULL,
                g INTEGER,
                s INTEGER,
                p INTEGER,
                credits REAL NOT NULL
            );
            """
        )
    elif version == 42:
        conn.executescript(
            """
            CREATE TABLE IF NOT EXISTS kpi_meta (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                next_report_at INTEGER NOT NULL,
                next_id INTEGER NOT NULL,
                period_trade REAL NOT NULL,
                period_contract REAL NOT NULL
            );
            CREATE TABLE IF NOT EXISTS kpi_targets (
                kpi TEXT NOT NULL,
                owner TEXT NOT NULL,
                value REAL NOT NULL,
                set_at INTEGER NOT NULL,
                PRIMARY KEY (kpi, owner)
            );
            CREATE TABLE IF NOT EXISTS kpi_reports (
                id INTEGER PRIMARY KEY,
                at INTEGER NOT NULL,
                pending INTEGER NOT NULL,
                highlights TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS kpi_measurements (
                report_id INTEGER NOT NULL,
                kpi TEXT NOT NULL,
                owner TEXT NOT NULL,
                value REAL NOT NULL,
                target REAL,
                event_ids TEXT NOT NULL,
                PRIMARY KEY (report_id, kpi, owner)
            );
            """
        )
    elif version == 43:
        conn.executescript(
            """
            CREATE TABLE IF NOT EXISTS incident_meta (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                next_id INTEGER NOT NULL,
                next_review_id INTEGER NOT NULL,
                next_risk_id INTEGER NOT NULL,
                next_review_at INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS incident_rows (
                id INTEGER PRIMARY KEY,
                kind TEXT NOT NULL,
                severity TEXT NOT NULL,
                status TEXT NOT NULL,
                opened_at INTEGER NOT NULL,
                objects TEXT NOT NULL,
                owner INTEGER,
                actions TEXT NOT NULL,
                resolution TEXT,
                closed_at INTEGER,
                event_ids TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS incident_reviews (
                id INTEGER PRIMARY KEY,
                incident_id INTEGER NOT NULL,
                at INTEGER NOT NULL,
                root_cause TEXT NOT NULL,
                timeline TEXT NOT NULL,
                follow_ups TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS incident_risks (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                likelihood REAL NOT NULL,
                impact REAL NOT NULL,
                review_at INTEGER NOT NULL,
                last_reviewed_at INTEGER
            );
            """
        )
    elif version == 44:
        conn.executescript(
            """
            CREATE TABLE IF NOT EXISTS maintenance_meta (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                next_id INTEGER NOT NULL,
                next_plan_id INTEGER NOT NULL,
                next_order_id INTEGER NOT NULL,
                next_reading_id INTEGER NOT NULL,
                next_prediction_id INTEGER NOT NULL,
                next_at INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS maintenance_assets (
                id INTEGER PRIMARY KEY,
                kind TEXT NOT NULL,
                key TEXT NOT NULL,
                galaxy INTEGER NOT NULL,
                system INTEGER NOT NULL,
                position INTEGER NOT NULL,
                condition REAL NOT NULL,
                hours_use REAL NOT NULL,
                installed_at INTEGER NOT NULL,
                last_service_at INTEGER NOT NULL,
                last_service_hours REAL NOT NULL,
                offline_until INTEGER
            );
            CREATE TABLE IF NOT EXISTS maintenance_plans (
                id INTEGER PRIMARY KEY,
                asset_id INTEGER NOT NULL,
                kind TEXT NOT NULL,
                interval REAL NOT NULL,
                enabled INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS maintenance_orders (
                id INTEGER PRIMARY KEY,
                asset_id INTEGER NOT NULL,
                kind TEXT NOT NULL,
                started_at INTEGER NOT NULL,
                ends_at INTEGER NOT NULL,
                iron REAL NOT NULL,
                silicon REAL NOT NULL
            );
            CREATE TABLE IF NOT EXISTS maintenance_readings (
                id INTEGER PRIMARY KEY,
                asset_id INTEGER NOT NULL,
                at INTEGER NOT NULL,
                temperature REAL NOT NULL,
                vibration REAL NOT NULL,
                power REAL NOT NULL,
                hours_use REAL NOT NULL
            );
            CREATE TABLE IF NOT EXISTS maintenance_predictions (
                id INTEGER PRIMARY KEY,
                asset_id INTEGER NOT NULL,
                at INTEGER NOT NULL,
                probability REAL NOT NULL,
                horizon_secs INTEGER NOT NULL,
                reading_id INTEGER NOT NULL
            );
            """
        )
    elif version == 45:
        for column, sql in [
            ("source", "ALTER TABLE spy_reports ADD COLUMN source TEXT NOT NULL DEFAULT 'probe'"),
            ("observed_at", "ALTER TABLE spy_reports ADD COLUMN observed_at INTEGER NOT NULL DEFAULT 0"),
            ("arrives_at", "ALTER TABLE spy_reports ADD COLUMN arrives_at INTEGER NOT NULL DEFAULT 0"),
            ("confidence", "ALTER TABLE spy_reports ADD COLUMN confidence REAL NOT NULL DEFAULT 1"),
            ("deceived", "ALTER TABLE spy_reports ADD COLUMN deceived INTEGER NOT NULL DEFAULT 0"),
        ]:
            add_column(conn, "spy_reports", column, sql)
        conn.execute("UPDATE spy_reports SET observed_at = created_at WHERE observed_at = 0")
        conn.execute("UPDATE spy_reports SET arrives_at = created_at WHERE arrives_at = 0")
    else:
        raise SystemExit(
            f"migrate_v{version} is not in this generator; extend it when SCHEMA_VERSION moves"
        )


def snapshot(conn: sqlite3.Connection, path: Path) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    if path.exists():
        path.unlink()
    dst = sqlite3.connect(path)
    try:
        conn.backup(dst)
        dst.execute("PRAGMA journal_mode=DELETE")
        dst.execute("VACUUM")
        dst.commit()
    finally:
        dst.close()


def write_too_new(path: Path) -> None:
    if path.exists():
        path.unlink()
    conn = sqlite3.connect(path)
    try:
        conn.execute("PRAGMA journal_mode=DELETE")
        conn.execute("PRAGMA user_version = 999")
        conn.execute("CREATE TABLE meta (id INTEGER PRIMARY KEY)")
        conn.commit()
        conn.execute("VACUUM")
    finally:
        conn.close()


def write_seeded_v9(
    path: Path,
    *,
    metal: float,
    crystal: float,
    deuterium: float,
    buildings: tuple[tuple[str, int], ...],
    ships: tuple[tuple[str, int], ...] = (),
) -> None:
    if path.exists():
        path.unlink()
    conn = sqlite3.connect(path)
    try:
        conn.execute("PRAGMA journal_mode=DELETE")
        for version in range(1, 10):
            migrate(conn, version)
        conn.execute(
            "INSERT INTO meta (id, last_tick_unix, rng_seed, galaxy_seed, home_kind, home_size, deterministic) "
            "VALUES (1, 0, 1, 1, 'rocky', 163, 1)"
        )
        conn.execute(
            "INSERT INTO resources (planet_id, metal, crystal, deuterium) VALUES (1, ?, ?, ?)",
            (metal, crystal, deuterium),
        )
        for kind, level in buildings:
            conn.execute(
                "INSERT INTO buildings (planet_id, kind, level) VALUES (1, ?, ?)",
                (kind, level),
            )
        for kind, count in ships:
            conn.execute(
                "INSERT INTO ships (planet_id, kind, count) VALUES (1, ?, ?)",
                (kind, count),
            )
        conn.execute("PRAGMA user_version = 9")
        conn.commit()
        conn.execute("VACUUM")
    finally:
        conn.close()


def write_expedition(path: Path) -> None:
    write_seeded_v9(
        path,
        metal=500,
        crystal=500,
        deuterium=10000,
        buildings=(
            ("metal_mine", 1),
            ("crystal_mine", 1),
            ("solar_plant", 1),
        ),
        ships=(("light_fighter", 10),),
    )


def write_build_research(path: Path) -> None:
    write_seeded_v9(
        path,
        metal=500,
        crystal=1500,
        deuterium=500,
        buildings=(
            ("metal_mine", 1),
            ("crystal_mine", 1),
            ("solar_plant", 1),
            ("research_lab", 1),
        ),
    )


def main() -> int:
    current = current_schema()
    if current < 2:
        print("SCHEMA_VERSION is 1; no older fixtures to keep", file=sys.stderr)
        return 1
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    conn = sqlite3.connect(":memory:")
    try:
        conn.execute("PRAGMA journal_mode=DELETE")
        for version in range(1, current):
            migrate(conn, version)
            if version == 1:
                conn.execute(
                    "INSERT INTO meta (id, last_tick_unix, rng_seed) VALUES (1, ?, 7)",
                    (FUTURE_TICK,),
                )
                conn.execute(
                    "INSERT INTO resources (planet_id, metal, crystal, deuterium) VALUES (1, 10, 20, 30)"
                )
                conn.execute(
                    "INSERT INTO buildings (planet_id, kind, level) VALUES (1, 'metal_mine', 4)"
                )
                conn.execute(
                    "INSERT INTO tech_levels (id, level) VALUES ('energy', 3)"
                )
            conn.execute(f"PRAGMA user_version = {version}")
            conn.commit()
            snapshot(conn, OUT_DIR / f"v{version}.sqlite")
            print(f"wrote {OUT_DIR.relative_to(ROOT)}/v{version}.sqlite")
    finally:
        conn.close()
    write_too_new(OUT_DIR / "too_new.sqlite")
    print(f"wrote {OUT_DIR.relative_to(ROOT)}/too_new.sqlite")
    write_expedition(OUT_DIR / "expedition.sqlite")
    print(f"wrote {OUT_DIR.relative_to(ROOT)}/expedition.sqlite")
    write_build_research(OUT_DIR / "build_research.sqlite")
    print(f"wrote {OUT_DIR.relative_to(ROOT)}/build_research.sqlite")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
