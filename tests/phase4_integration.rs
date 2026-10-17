//! Integration tests for Phase 4 of `watchout_pixls`:
//! 1. Data-Driven Classes, Weapons, and Multi-Stage Weapon Evolution (RON/JSON & Mods).
//! 2. XP System, Leveling, Attribute Points, and Branching Skilltree (Passive Stats & Active Skills).
//! 3. Standard Enemy State-Machine AI (`Patrol`, `Chase`, `Attack`) and Multi-Phase Level-Gated Endboss.
//! 4. Auto-Save (fixed intervals & zone transitions), Map Checkpoints with Death/Respawn,
//!    and Pause Menu Manual Save/Load.

use bevy::prelude::*;
use watchout_pixls::build_watchout_app;
use watchout_pixls::combat::{
    evaluate_enemy_ai_state, simulate_boss_encounter, BossAttackPattern, BossController, BossPhase,
    CombatOutcomeEvent, EnemyAiState, EnemyController, HealthPool, PlayerCombatActionEvent,
};
use watchout_pixls::content::{ContentDefinition, DataFormat, GameContentRegistries, WeaponTier};
use watchout_pixls::palette_sprite::WorldPixelPos;
use watchout_pixls::progression::{
    AttributeStat, CharacterProgression, CombatStats, ProgressionError, SkillBranch, SkillTree,
    ATTRIBUTE_POINTS_PER_LEVEL, SKILL_POINTS_PER_LEVEL,
};
use watchout_pixls::save_checkpoint::{
    ActiveCheckpoint, AutoSaveConfig, CheckpointActivatedEvent, GameLoadedEvent, GameSaveState,
    GameSavedEvent, LivePlayerSnapshot, PauseMenuAction, PauseMenuOption, PauseMenuState,
    PlayerRespawnedEvent, SaveGameManager, AUTOSAVE_SLOT_NAME,
};
use watchout_pixls::scene::SceneTransitionEvent;
use watchout_pixls::state::{GameState, HeroController, StateTransitionAction};
use watchout_pixls::world::{HeroMoveEvent, TileCoord, WallKind, WorldGridConfig, WorldMap};

#[test]
fn test_phase4_data_driven_classes_weapons_and_evolution_paths() {
    let mut app = build_watchout_app();
    app.update();

    let mut registries = app.world().resource::<GameContentRegistries>().clone();
    assert!(registries.validate_weapon_evolution_paths().is_ok());

    // 1. Verify data-driven classes and weapons loaded from RON files
    assert!(registries.classes.get("class.pixel_ranger").is_some());
    assert!(registries.classes.get("class.pastel_alchemist").is_some());
    assert!(registries.classes.get("class.blade_vanguard").is_some());
    assert!(registries.classes.get("class.cyber_sentinel").is_some());

    // 2. Verify weapon evolution chain from Rusty Dagger -> Iron Shortsword -> Cobalt Katana -> Laser Sword
    let chain = registries.trace_weapon_evolution_path("weapon.rusty_dagger");
    assert_eq!(
        chain,
        vec![
            "weapon.rusty_dagger".to_string(),
            "weapon.iron_shortsword".to_string(),
            "weapon.cobalt_katana".to_string(),
            "weapon.laser_sword".to_string(),
        ]
    );

    let dagger = registries.weapons.get("weapon.rusty_dagger").unwrap();
    assert_eq!(dagger.tier, WeaponTier::Starter);
    let laser = registries.weapons.get("weapon.laser_sword").unwrap();
    assert_eq!(laser.tier, WeaponTier::EndgameSciFi);
    assert!(laser.base_damage > dagger.base_damage * 5);

    // 3. Verify adding a brand-new modded class and 2-stage weapon evolution path via JSON without code changes
    let mod_class_json = r#"
        {
            "id": "class.chrono_duelist",
            "display_name": "Chrono Duelist",
            "base_health": 115,
            "base_mana": 65,
            "base_speed": 68.0,
            "starting_items": ["weapon.rusted_rapier", "item.berry_potion"],
            "passive_trait": "VanguardEdge"
        }
    "#;
    let mod_weapons_json = r#"
        [
            {
                "id": "weapon.rusted_rapier",
                "display_name": "Rusted Clockwork Rapier",
                "tier": "Starter",
                "base_damage": 9,
                "attack_speed": 1.6,
                "range_px": 20.0,
                "crit_chance": 0.07,
                "required_level": 1,
                "evolves_to": ["weapon.tachyon_lance"],
                "sprite_id": "sprite_rusted_rapier"
            },
            {
                "id": "weapon.tachyon_lance",
                "display_name": "Tachyon Starlight Lance",
                "tier": "EndgameSciFi",
                "base_damage": 68,
                "attack_speed": 2.4,
                "range_px": 36.0,
                "crit_chance": 0.28,
                "required_level": 4,
                "evolves_to": [],
                "sprite_id": "sprite_tachyon_lance"
            }
        ]
    "#;

    registries
        .classes
        .load_single_from_str(mod_class_json, DataFormat::Json)
        .unwrap();
    registries
        .weapons
        .load_list_from_str(mod_weapons_json, DataFormat::Json)
        .unwrap();

    assert_eq!(
        registries
            .classes
            .get("class.chrono_duelist")
            .unwrap()
            .display_name(),
        "Chrono Duelist"
    );

    // 4. Test weapon evolution progression & level requirements on CharacterProgression
    let mut prog = CharacterProgression::from_class("class.blade_vanguard", &registries).unwrap();
    assert_eq!(prog.equipped_weapon_id, "weapon.rusty_dagger");
    assert_eq!(prog.level, 1);

    // Evolving Rusty Dagger -> Iron Shortsword requires level 2, so at level 1 it must fail
    assert_eq!(
        prog.evolve_weapon(None, &registries),
        Err(ProgressionError::InsufficientLevelForWeapon {
            weapon_id: "weapon.iron_shortsword".to_string(),
            required_level: 2,
            current_level: 1,
        })
    );

    // Gain 100 XP -> Level 2, now evolve to Iron Shortsword
    assert_eq!(prog.gain_xp(100), 1);
    assert_eq!(prog.level, 2);
    let evolved_stage_2 = prog.evolve_weapon(None, &registries).unwrap();
    assert_eq!(evolved_stage_2, "weapon.iron_shortsword");

    // Iron Shortsword has two branches: "weapon.cobalt_katana" and "weapon.arc_blade" (both require level 4)
    // Trying to evolve to an unrelated weapon must fail with InvalidEvolutionTarget
    assert_eq!(
        prog.evolve_weapon(Some("weapon.quantum_staff"), &registries),
        Err(ProgressionError::InvalidEvolutionTarget {
            current_weapon: "weapon.iron_shortsword".to_string(),
            target_weapon: "weapon.quantum_staff".to_string(),
        })
    );

    // Level up to Level 5 (gain 200 + 300 + 400 = 900 XP)
    assert_eq!(prog.gain_xp(900), 3);
    assert_eq!(prog.level, 5);

    // Choose the "weapon.arc_blade" intermediate branch, then evolve to Endgame "weapon.laser_sword"!
    let evolved_stage_3 = prog
        .evolve_weapon(Some("weapon.arc_blade"), &registries)
        .unwrap();
    assert_eq!(evolved_stage_3, "weapon.arc_blade");

    let evolved_endgame = prog.evolve_weapon(None, &registries).unwrap();
    assert_eq!(evolved_endgame, "weapon.laser_sword");
    assert_eq!(
        prog.current_weapon(&registries).unwrap().tier,
        WeaponTier::EndgameSciFi
    );

    // Further evolution beyond Endgame weapon returns NoFurtherWeaponEvolution
    assert_eq!(
        prog.evolve_weapon(None, &registries),
        Err(ProgressionError::NoFurtherWeaponEvolution(
            "weapon.laser_sword".to_string()
        ))
    );
}

#[test]
fn test_phase4_xp_leveling_attributes_and_branching_skilltree() {
    let registries = {
        let mut r = GameContentRegistries::new();
        r.register_core_defaults().unwrap();
        r
    };
    let skill_tree = SkillTree::default_branching_tree();

    // Verify all 3 branches exist and have multiple nodes
    assert!(skill_tree.nodes_in_branch(SkillBranch::Offense).len() >= 4);
    assert!(skill_tree.nodes_in_branch(SkillBranch::Mobility).len() >= 4);
    assert!(skill_tree.nodes_in_branch(SkillBranch::Defense).len() >= 4);

    let mut prog = CharacterProgression::from_class("class.blade_vanguard", &registries).unwrap();
    let base_stats = prog.compute_effective_stats(&registries, &skill_tree);

    // 1. Attempting to unlock a child branch node before its prerequisite root node must fail
    assert_eq!(
        prog.unlock_skill("skill.active_dash", &skill_tree),
        Err(ProgressionError::InsufficientLevelForSkill {
            skill_id: "skill.active_dash".to_string(),
            required_level: 2,
            current_level: 1,
        })
    );

    // 2. Gain XP to reach Level 4 (100 + 200 + 300 = 600 XP -> +3 levels)
    let gained = prog.gain_xp(650);
    assert_eq!(gained, 3);
    assert_eq!(prog.level, 4);
    assert_eq!(prog.current_xp, 50);
    assert_eq!(
        prog.unspent_attribute_points,
        3 * ATTRIBUTE_POINTS_PER_LEVEL
    );
    // 1 starter skill point + 3 from level ups = 4 skill points
    assert_eq!(prog.unspent_skill_points, 1 + 3 * SKILL_POINTS_PER_LEVEL);

    // Even at Level 4, unlocking "skill.active_dash" without "skill.mobility_root" fails due to prerequisite
    assert_eq!(
        prog.unlock_skill("skill.active_dash", &skill_tree),
        Err(ProgressionError::MissingSkillPrerequisites {
            skill_id: "skill.active_dash".to_string(),
            missing: vec!["skill.mobility_root".to_string()],
        })
    );

    // 3. Unlock Mobility Root (+8 Speed) -> then unlock Active Dash -> then Offense Root (+6 Attack) -> Active AoE Strike
    prog.unlock_skill("skill.mobility_root", &skill_tree)
        .unwrap();
    prog.unlock_skill("skill.active_dash", &skill_tree).unwrap();
    prog.unlock_skill("skill.offense_root", &skill_tree)
        .unwrap();
    prog.unlock_skill("skill.active_aoe_strike", &skill_tree)
        .unwrap();

    assert!(prog.unlocked_dash_skill(&skill_tree).is_some());
    assert!(prog.unlocked_aoe_skill(&skill_tree).is_some());
    assert_eq!(prog.unspent_skill_points, 0);

    // 4. Allocate Attribute Points into Attack, Speed, Defense, and Vitality
    prog.allocate_attribute(AttributeStat::Attack, 4).unwrap();
    prog.allocate_attribute(AttributeStat::Speed, 2).unwrap();
    prog.allocate_attribute(AttributeStat::Defense, 2).unwrap();
    prog.allocate_attribute(AttributeStat::Vitality, 1).unwrap();
    assert_eq!(prog.unspent_attribute_points, 0);

    let upgraded_stats = prog.compute_effective_stats(&registries, &skill_tree);
    assert!(upgraded_stats.attack > base_stats.attack + 20);
    assert!(upgraded_stats.speed > base_stats.speed + 10.0);
    assert!(upgraded_stats.defense > base_stats.defense + 10);
    assert!(upgraded_stats.max_health > base_stats.max_health + 80);
}

#[test]
fn test_phase4_enemy_state_machine_ai_and_active_skills_in_ecs() {
    // 1. Unit-verify pure state machine transitions: Patrol -> Chase -> Attack -> Chase -> Patrol
    let aggro = 60.0;
    let attack = 16.0;
    let leash = 90.0;

    let s1 = evaluate_enemy_ai_state(EnemyAiState::Patrol, 80.0, aggro, attack, leash);
    assert_eq!(s1, EnemyAiState::Patrol);

    let s2 = evaluate_enemy_ai_state(s1, 45.0, aggro, attack, leash);
    assert_eq!(s2, EnemyAiState::Chase);

    let s3 = evaluate_enemy_ai_state(s2, 12.0, aggro, attack, leash);
    assert_eq!(s3, EnemyAiState::Attack);

    let s4 = evaluate_enemy_ai_state(s3, 30.0, aggro, attack, leash);
    assert_eq!(s4, EnemyAiState::Chase);

    // Hysteresis: at distance 75.0 (> aggro 60.0, but <= leash 90.0), enemy continues chasing
    let s5 = evaluate_enemy_ai_state(s4, 75.0, aggro, attack, leash);
    assert_eq!(s5, EnemyAiState::Chase);

    // Beyond leash range (100.0 > 90.0), enemy returns to Patrol
    let s6 = evaluate_enemy_ai_state(s5, 100.0, aggro, attack, leash);
    assert_eq!(s6, EnemyAiState::Patrol);

    // 2. Live ECS verification of Enemy AI states, Active Dash, and Active AoE Attack
    let mut app = build_watchout_app();
    app.update();
    app.world_mut().send_event(StateTransitionAction::StartGame);
    app.update();
    app.update();

    // Unlock Dash and AoE skills on the Hero
    {
        let skill_tree = app.world().resource::<SkillTree>().clone();
        let mut query = app
            .world_mut()
            .query_filtered::<&mut CharacterProgression, With<HeroController>>();
        let mut prog = query.single_mut(app.world_mut());
        prog.gain_xp(600); // Level 4
        prog.unlock_skill("skill.mobility_root", &skill_tree)
            .unwrap();
        prog.unlock_skill("skill.active_dash", &skill_tree).unwrap();
        prog.unlock_skill("skill.offense_root", &skill_tree)
            .unwrap();
        prog.unlock_skill("skill.active_aoe_strike", &skill_tree)
            .unwrap();
    }
    app.update();

    // Execute Active Dash Skill (Facing South from (152, 112))
    let pos_before_dash = app
        .world_mut()
        .query_filtered::<&WorldPixelPos, With<HeroController>>()
        .single(app.world())
        .to_vec2();

    app.world_mut().send_event(PlayerCombatActionEvent::UseDash);
    app.update();

    let (pos_after_dash, invuln_after_dash) = {
        let mut q = app
            .world_mut()
            .query_filtered::<(&WorldPixelPos, &CombatStats), With<HeroController>>();
        let (p, s) = q.single(app.world());
        (p.to_vec2(), s.invulnerability_timer)
    };
    assert!(
        pos_after_dash.y > pos_before_dash.y + 10.0,
        "Dash skill must displace player forward along facing direction"
    );
    assert!(
        invuln_after_dash > 0.0,
        "Dash skill must grant brief invulnerability"
    );

    // Teleport Hero within aggro range (40px) of the Mint Slime at (48.0, 192.0) -> Hero at (48.0, 156.0)
    app.world_mut()
        .send_event(HeroMoveEvent::TeleportToPixel(Vec2::new(48.0, 156.0)));
    app.update();
    app.update();

    // Mint Slime must transition from Patrol -> Chase
    let slime_state = app
        .world_mut()
        .query::<&EnemyController>()
        .iter(app.world())
        .find(|e| e.entity_id == "entity.mint_slime")
        .unwrap()
        .ai_state;
    assert_eq!(slime_state, EnemyAiState::Chase);

    // Now use Active AoE Skill (`UseAreaAttack`, radius 48px) to hit and defeat the Mint Slime!
    app.world_mut()
        .send_event(PlayerCombatActionEvent::UseAreaAttack);
    app.update();

    let slime_count_after = app
        .world_mut()
        .query::<&EnemyController>()
        .iter(app.world())
        .filter(|e| e.entity_id == "entity.mint_slime")
        .count();
    assert_eq!(
        slime_count_after, 0,
        "Mint Slime within AoE radius must be defeated and despawned"
    );
}

#[test]
fn test_phase4_endboss_phases_patterns_and_level_gate_balancing() {
    let mut registries = GameContentRegistries::new();
    registries.register_core_defaults().unwrap();
    let skill_tree = SkillTree::default_branching_tree();
    let boss = BossController::chrono_overlord();
    let boss_max_hp = 650;

    // 1. Verify Boss Phase transitions and pattern rotations
    let mut test_boss = boss.clone();
    let mut test_hp = HealthPool::new(100);
    assert_eq!(test_boss.phase, BossPhase::Phase1);
    assert_eq!(
        test_boss.next_attack_pattern(),
        BossAttackPattern::CleavingStrike
    );
    assert_eq!(
        test_boss.next_attack_pattern(),
        BossAttackPattern::RadialPulse
    );

    // Drop HP to 55% -> transitions to Phase 2
    test_hp.apply_damage(45);
    assert_eq!(test_boss.sync_phase(&test_hp), Some(BossPhase::Phase2));
    assert_eq!(
        test_boss.next_attack_pattern(),
        BossAttackPattern::ChargingLunge
    );

    // Drop HP to 20% -> transitions to Phase 3
    test_hp.apply_damage(35);
    assert_eq!(test_boss.sync_phase(&test_hp), Some(BossPhase::Phase3));
    assert_eq!(
        test_boss.next_attack_pattern(),
        BossAttackPattern::SupernovaBurst
    );

    // 2. Verify Level 1 starter character CANNOT realistically defeat the Endboss
    let starter_hero =
        CharacterProgression::from_class("class.blade_vanguard", &registries).unwrap();
    assert_eq!(starter_hero.level, 1);
    let report_lvl1 =
        simulate_boss_encounter(&starter_hero, &registries, &skill_tree, &boss, boss_max_hp);
    assert!(
        !report_lvl1.player_won,
        "Level 1 character with Rusty Dagger must NOT defeat the Endboss"
    );
    assert_eq!(report_lvl1.player_remaining_hp, 0);
    assert!(
        report_lvl1.boss_remaining_hp > 600,
        "Boss should barely take scratch damage against a Level 1 character (remaining HP: {})",
        report_lvl1.boss_remaining_hp
    );

    // 3. Verify a Level 6 progressed character (with evolved Laser Sword, attributes, and skills)
    // realistically defeats the Endboss across all 3 phases!
    let mut leveled_hero =
        CharacterProgression::from_class("class.blade_vanguard", &registries).unwrap();
    leveled_hero.gain_xp(1500); // Level 1 -> Level 6 (100+200+300+400+500 = 1500 XP)
    assert_eq!(leveled_hero.level, 6);

    // Evolve weapon all the way from Rusty Dagger -> Iron Shortsword -> Cobalt Katana -> Laser Sword
    leveled_hero.evolve_weapon(None, &registries).unwrap();
    leveled_hero.evolve_weapon(None, &registries).unwrap();
    leveled_hero.evolve_weapon(None, &registries).unwrap();
    assert_eq!(leveled_hero.equipped_weapon_id, "weapon.laser_sword");

    // Spend attribute points
    leveled_hero
        .allocate_attribute(AttributeStat::Attack, 8)
        .unwrap();
    leveled_hero
        .allocate_attribute(AttributeStat::Defense, 4)
        .unwrap();
    leveled_hero
        .allocate_attribute(AttributeStat::Vitality, 3)
        .unwrap();

    // Unlock Offense + Mobility + Defense skilltree nodes including Active AoE and Dash
    for skill_id in [
        "skill.offense_root",
        "skill.active_aoe_strike",
        "skill.offense_capstone",
        "skill.mobility_root",
        "skill.active_dash",
        "skill.defense_root",
    ] {
        leveled_hero.unlock_skill(skill_id, &skill_tree).unwrap();
    }

    let report_lvl6 =
        simulate_boss_encounter(&leveled_hero, &registries, &skill_tree, &boss, boss_max_hp);
    assert!(
        report_lvl6.player_won,
        "Level 6 progressed character with Laser Sword and skills must defeat the Endboss"
    );
    assert!(report_lvl6.player_remaining_hp > 0);
    assert_eq!(report_lvl6.boss_remaining_hp, 0);
    assert_eq!(report_lvl6.highest_phase_reached, BossPhase::Phase3);
}

#[test]
fn test_phase4_autosave_checkpoints_respawn_and_pause_menu_save_load() {
    let mut app = build_watchout_app();

    // Configure rapid interval auto-save (every 3 ticks) for deterministic test verification
    {
        let mut auto_cfg = app.world_mut().resource_mut::<AutoSaveConfig>();
        auto_cfg.interval_ticks = 3;
        auto_cfg.interval_secs = 999.0;
    }

    app.update();
    app.world_mut().send_event(StateTransitionAction::StartGame);
    app.update();
    app.update();

    // 1. Run 3 InGame ticks and verify fixed-interval Auto-Save fires
    app.update();
    app.update();
    app.update();

    {
        let save_mgr = app.world().resource::<SaveGameManager>();
        assert!(
            save_mgr.auto_interval_saves >= 1,
            "Auto-save must trigger automatically after interval_ticks"
        );
        assert!(save_mgr.has_slot(AUTOSAVE_SLOT_NAME));
    }

    // 2. Trigger a Zone Transition (enter starter house prefab) and verify ZoneTransition Auto-Save fires
    app.world_mut()
        .send_event(SceneTransitionEvent::EnterPrefab {
            prefab_path: "assets/prefabs/starter_house.ron".to_string(),
            return_position: Some(Vec2::new(176.0, 112.0)),
        });
    app.update();

    {
        let save_mgr = app.world().resource::<SaveGameManager>();
        assert!(
            save_mgr.zone_transition_saves >= 1,
            "Auto-save must trigger automatically upon zone transition"
        );
    }

    // Exit back to Overworld
    app.world_mut()
        .send_event(SceneTransitionEvent::ExitToOverworld);
    app.update();

    // 3. Test Map Checkpoint activation & Respawn after Death
    // Teleport Hero to the Western Meadow Waystone checkpoint at (96.0, 144.0)
    app.world_mut()
        .send_event(HeroMoveEvent::TeleportToPixel(Vec2::new(96.0, 144.0)));
    app.update();

    {
        let active_cp = app.world().resource::<ActiveCheckpoint>();
        assert_eq!(active_cp.checkpoint_id, "checkpoint.west_shrine");
        assert_eq!(active_cp.respawn_position, Vec2::new(96.0, 144.0));
    }

    // Move Hero away to (200.0, 200.0) and reduce HP to 0 (simulating lethal combat damage)
    app.world_mut()
        .send_event(HeroMoveEvent::TeleportToPixel(Vec2::new(200.0, 200.0)));
    app.update();
    {
        let mut q = app
            .world_mut()
            .query_filtered::<&mut CombatStats, With<HeroController>>();
        let mut stats = q.single_mut(app.world_mut());
        stats.current_health = 0;
    }
    app.update(); // player_death_and_respawn_system detects HP <= 0 and respawns at checkpoint!

    {
        let mut q = app
            .world_mut()
            .query_filtered::<(&WorldPixelPos, &CombatStats), With<HeroController>>();
        let (pos, stats) = q.single(app.world());
        assert_eq!(
            pos.to_vec2(),
            Vec2::new(96.0, 144.0),
            "Hero must respawn at the active map checkpoint after dying"
        );
        assert_eq!(
            stats.current_health, stats.max_health,
            "Hero health must be restored upon checkpoint respawn"
        );
        assert_eq!(
            app.world()
                .resource::<ActiveCheckpoint>()
                .death_respawn_count,
            1
        );
    }

    // 4. Test Pause Menu Manual Save & Load (including disk file roundtrip + world chunk modification)
    let tmp_save_file = std::env::temp_dir().join("watchout_pixls_phase4_manual_save.ron");
    let grid = *app.world().resource::<WorldGridConfig>();

    // Modify progression (gain XP to Level 3, evolve weapon to Iron Shortsword) and modify a world tile
    {
        let registries = app.world().resource::<GameContentRegistries>().clone();
        let mut q = app
            .world_mut()
            .query_filtered::<&mut CharacterProgression, With<HeroController>>();
        let mut prog = q.single_mut(app.world_mut());
        prog.gain_xp(300); // Level 3
        prog.evolve_weapon(None, &registries).unwrap();
        assert_eq!(prog.level, 3);
        assert_eq!(prog.equipped_weapon_id, "weapon.iron_shortsword");
    }
    {
        let mut world_map = app.world_mut().resource_mut::<WorldMap>();
        assert!(world_map.set_wall(TileCoord::new(4, 4), &grid, Some(WallKind::AncientPillar)));
    }

    // Enter GameState::Paused
    app.world_mut()
        .send_event(StateTransitionAction::TogglePause);
    app.update();
    app.update();
    assert_eq!(
        *app.world().resource::<State<GameState>>().get(),
        GameState::Paused
    );

    // Navigate Pause Menu cursor from Resume (0) -> ManualSave (1) and confirm selection
    {
        let mut pause_menu = app.world_mut().resource_mut::<PauseMenuState>();
        pause_menu.save_file_path = Some(tmp_save_file.clone());
    }
    app.world_mut().send_event(PauseMenuAction::MoveCursor(1));
    app.world_mut()
        .send_event(PauseMenuAction::ConfirmSelection);
    app.update();

    assert_eq!(
        app.world().resource::<PauseMenuState>().selected_option(),
        PauseMenuOption::ManualSave
    );
    assert_eq!(app.world().resource::<SaveGameManager>().manual_saves, 1);
    assert!(tmp_save_file.exists());

    // Now corrupt/alter the in-memory LivePlayerSnapshot while Paused (move to (16.0, 16.0), reset level to 1, equip rusty_dagger)
    {
        let mut live = app.world_mut().resource_mut::<LivePlayerSnapshot>();
        live.hero_position = Vec2::new(16.0, 16.0);
        live.progression.level = 1;
        live.progression.equipped_weapon_id = "weapon.rusty_dagger".to_string();
        live.combat_stats.current_health = 12;
    }

    // Navigate Pause Menu cursor from ManualSave (1) -> ManualLoad (2) and confirm selection
    app.world_mut().send_event(PauseMenuAction::MoveCursor(1));
    app.world_mut()
        .send_event(PauseMenuAction::ConfirmSelection);
    app.update();

    assert_eq!(
        app.world().resource::<PauseMenuState>().selected_option(),
        PauseMenuOption::ManualLoad
    );
    assert_eq!(app.world().resource::<SaveGameManager>().manual_loads, 1);

    // Resume from Paused -> InGame via Pause Menu Resume (or TogglePause)
    app.world_mut()
        .send_event(StateTransitionAction::TogglePause);
    app.update();
    app.update();
    assert_eq!(
        *app.world().resource::<State<GameState>>().get(),
        GameState::InGame
    );

    // Verify full restoration of Hero position, progression, weapon, health, and world tile modification in InGame
    {
        let mut q = app.world_mut().query_filtered::<
            (&WorldPixelPos, &CharacterProgression, &CombatStats),
            With<HeroController>,
        >();
        let (pos, prog, stats) = q.single(app.world());
        assert_eq!(pos.to_vec2(), Vec2::new(96.0, 144.0));
        assert_eq!(prog.level, 3);
        assert_eq!(prog.equipped_weapon_id, "weapon.iron_shortsword");
        assert_eq!(stats.current_health, stats.max_health);

        let world_map = app.world().resource::<WorldMap>();
        let restored_tile = world_map.get_tile(TileCoord::new(4, 4), &grid).unwrap();
        assert_eq!(restored_tile.wall, Some(WallKind::AncientPillar));
    }

    // Also verify JSON roundtrip of GameSaveState
    let loaded_from_disk = GameSaveState::load_from_file(&tmp_save_file).unwrap();
    let json_str = loaded_from_disk.to_string_pretty(DataFormat::Json).unwrap();
    let decoded_json = GameSaveState::from_str_format(&json_str, DataFormat::Json).unwrap();
    assert_eq!(decoded_json.progression.level, 3);
    assert_eq!(
        decoded_json.progression.equipped_weapon_id,
        "weapon.iron_shortsword"
    );

    let _ = std::fs::remove_file(&tmp_save_file);

    // Suppress unused import warnings in case any event type is only checked indirectly
    let _ = (
        std::marker::PhantomData::<CombatOutcomeEvent>,
        std::marker::PhantomData::<CheckpointActivatedEvent>,
        std::marker::PhantomData::<PlayerRespawnedEvent>,
        std::marker::PhantomData::<GameSavedEvent>,
        std::marker::PhantomData::<GameLoadedEvent>,
    );
}
