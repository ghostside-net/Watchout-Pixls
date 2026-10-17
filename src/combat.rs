//! Combat System, Standard Enemy State-Machine AI, Active Skills (`Dash`, `AreaAttack`),
//! and Multi-Phase Endboss Mechanics (Phase 4).
//!
//! Provides:
//! - [`EnemyAiState`] (`Patrol`, `Chase`, `Attack`) and [`EnemyController`] for standard enemies.
//! - [`BossPhase`] (`Phase1`, `Phase2`, `Phase3`), [`BossAttackPattern`], and [`BossController`]
//!   for the level-gated Endboss encounter.
//! - Active skill execution (`Dash` with collision-safe displacement + invulnerability, and
//!   `AreaAttack` radial AoE damage) and basic weapon attacks.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::palette_sprite::WorldPixelPos;
use crate::player::{
    compute_normalized_movement_delta, resolve_subtile_movement, FacingDirection, PlayerHitbox,
    PlayerInputState, PlayerMovement,
};
use crate::progression::{
    CharacterProgression, CombatStats, LevelUpEvent, SkillTree, ATTRIBUTE_POINTS_PER_LEVEL,
    SKILL_POINTS_PER_LEVEL,
};
use crate::scene::SceneManager;
use crate::state::HeroController;
use crate::world::coords::{WorldGridConfig, WorldPos};
use crate::world::streaming::WorldMap;

/// Health pool component attached to hostile monsters, bosses, or destructibles.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthPool {
    pub current: i32,
    pub max: i32,
}

impl HealthPool {
    #[must_use]
    pub fn new(max: i32) -> Self {
        let clamped = max.max(1);
        Self {
            current: clamped,
            max: clamped,
        }
    }

    #[must_use]
    pub const fn is_alive(self) -> bool {
        self.current > 0
    }

    #[must_use]
    pub fn ratio(self) -> f32 {
        if self.max <= 0 {
            0.0
        } else {
            (self.current.max(0) as f32) / (self.max as f32)
        }
    }

    /// Subtracts `amount` (clamped `>= 0`) from `current` and returns actual damage dealt.
    pub fn apply_damage(&mut self, amount: i32) -> i32 {
        let dmg = amount.max(0);
        let before = self.current;
        self.current = (self.current - dmg).max(0);
        before - self.current
    }

    /// Restores `amount` health up to `max`.
    pub fn heal(&mut self, amount: i32) {
        if amount > 0 {
            self.current = (self.current + amount).min(self.max);
        }
    }
}

// ============================================================================
// 1. Standard Enemy Finite State Machine AI (Patrol -> Chase -> Attack)
// ============================================================================

/// Finite State Machine state for standard hostile enemies.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum EnemyAiState {
    /// Patrols between waypoints until the player enters `aggro_range_px`.
    #[default]
    Patrol,
    /// Pursues the player while within `leash_range_px`.
    Chase,
    /// Executes melee/ranged strikes while the player is within `attack_range_px`.
    Attack,
}

/// Pure function computing the next [`EnemyAiState`] from the current state and Euclidean distance
/// to the player.
#[must_use]
pub fn evaluate_enemy_ai_state(
    current: EnemyAiState,
    dist_to_player: f32,
    aggro_range_px: f32,
    attack_range_px: f32,
    leash_range_px: f32,
) -> EnemyAiState {
    if dist_to_player <= attack_range_px {
        return EnemyAiState::Attack;
    }

    let effective_leash = leash_range_px.max(aggro_range_px);
    match current {
        EnemyAiState::Patrol => {
            if dist_to_player <= aggro_range_px {
                EnemyAiState::Chase
            } else {
                EnemyAiState::Patrol
            }
        }
        EnemyAiState::Chase | EnemyAiState::Attack => {
            if dist_to_player <= effective_leash {
                EnemyAiState::Chase
            } else {
                EnemyAiState::Patrol
            }
        }
    }
}

/// ECS Component controlling a standard enemy's Patrol/Chase/Attack state machine and combat stats.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnemyController {
    pub entity_id: String,
    pub display_name: String,
    pub ai_state: EnemyAiState,
    pub patrol_waypoints: Vec<Vec2>,
    pub current_waypoint_index: usize,
    pub patrol_speed: f32,
    pub chase_speed: f32,
    pub aggro_range_px: f32,
    pub attack_range_px: f32,
    pub leash_range_px: f32,
    pub attack_damage: i32,
    pub defense: i32,
    pub attack_cooldown_secs: f32,
    pub attack_timer: f32,
    pub xp_reward: u32,
    pub drop_table: Vec<String>,
}

impl EnemyController {
    /// Creates a standard Mint Slime enemy patrolling between `waypoint_a` and `waypoint_b`.
    #[must_use]
    pub fn mint_slime(waypoint_a: Vec2, waypoint_b: Vec2) -> Self {
        Self {
            entity_id: "entity.mint_slime".to_string(),
            display_name: "Mint Slime".to_string(),
            ai_state: EnemyAiState::Patrol,
            patrol_waypoints: vec![waypoint_a, waypoint_b],
            current_waypoint_index: 0,
            patrol_speed: 24.0,
            chase_speed: 36.0,
            aggro_range_px: 56.0,
            attack_range_px: 14.0,
            leash_range_px: 84.0,
            attack_damage: 10,
            defense: 2,
            attack_cooldown_secs: 1.0,
            attack_timer: 0.0,
            xp_reward: 55,
            drop_table: vec!["item.berry_potion".to_string()],
        }
    }

    /// Creates a Cobalt Goblin Scout enemy patrolling a loop of waypoints.
    #[must_use]
    pub fn cobalt_goblin(waypoints: Vec<Vec2>) -> Self {
        Self {
            entity_id: "entity.cobalt_goblin".to_string(),
            display_name: "Cobalt Goblin Scout".to_string(),
            ai_state: EnemyAiState::Patrol,
            patrol_waypoints: waypoints,
            current_waypoint_index: 0,
            patrol_speed: 30.0,
            chase_speed: 44.0,
            aggro_range_px: 64.0,
            attack_range_px: 16.0,
            leash_range_px: 96.0,
            attack_damage: 15,
            defense: 4,
            attack_cooldown_secs: 0.9,
            attack_timer: 0.0,
            xp_reward: 85,
            drop_table: vec!["item.berry_potion".to_string()],
        }
    }
}

// ============================================================================
// 2. Endboss with Multi-Phase State Machine & Level-Gated Balancing
// ============================================================================

/// Combat phase of the Endboss, transitioning automatically as health thresholds are crossed.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
pub enum BossPhase {
    /// Phase 1 (`100%` down to `> 60%` HP): Measured tactical strikes (`CleavingStrike`, `RadialPulse`).
    #[default]
    Phase1,
    /// Phase 2 (`60%` down to `> 25%` HP): Enraged assault adding `ChargingLunge` and faster cadence.
    Phase2,
    /// Phase 3 (`<= 25%` HP): Overdrive desperation phase unleashing `SupernovaBurst`.
    Phase3,
}

impl BossPhase {
    /// Determines the appropriate [`BossPhase`] for a health ratio in `[0.0, 1.0]`.
    #[must_use]
    pub fn from_health_ratio(ratio: f32) -> Self {
        if ratio <= 0.25 {
            Self::Phase3
        } else if ratio <= 0.60 {
            Self::Phase2
        } else {
            Self::Phase1
        }
    }

    /// Attack damage multiplier for this phase.
    #[must_use]
    pub const fn damage_multiplier(self) -> f32 {
        match self {
            Self::Phase1 => 1.0,
            Self::Phase2 => 1.25,
            Self::Phase3 => 1.5,
        }
    }

    /// Movement speed multiplier for this phase.
    #[must_use]
    pub const fn speed_multiplier(self) -> f32 {
        match self {
            Self::Phase1 => 1.0,
            Self::Phase2 => 1.3,
            Self::Phase3 => 1.6,
        }
    }

    /// Attack cooldown multiplier (`< 1.0` means faster attacks in later phases).
    #[must_use]
    pub const fn cooldown_multiplier(self) -> f32 {
        match self {
            Self::Phase1 => 1.0,
            Self::Phase2 => 0.82,
            Self::Phase3 => 0.68,
        }
    }

    /// Ordered attack pattern rotation used by the boss in this phase.
    #[must_use]
    pub const fn pattern_rotation(self) -> &'static [BossAttackPattern] {
        match self {
            Self::Phase1 => &[
                BossAttackPattern::CleavingStrike,
                BossAttackPattern::RadialPulse,
            ],
            Self::Phase2 => &[
                BossAttackPattern::ChargingLunge,
                BossAttackPattern::CleavingStrike,
                BossAttackPattern::RadialPulse,
            ],
            Self::Phase3 => &[
                BossAttackPattern::SupernovaBurst,
                BossAttackPattern::ChargingLunge,
                BossAttackPattern::RadialPulse,
                BossAttackPattern::CleavingStrike,
            ],
        }
    }
}

/// Distinct boss attack patterns executed across [`BossPhase`]s.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BossAttackPattern {
    /// Heavy close-range strike.
    CleavingStrike,
    /// Medium-radius shockwave ring around the boss.
    RadialPulse,
    /// High-speed gap-closing lunge toward the player followed by a strike.
    ChargingLunge,
    /// Massive Phase 3 energy burst covering a wide radius.
    SupernovaBurst,
}

impl BossAttackPattern {
    /// Effective reach or AoE radius (in pixels) of this attack pattern.
    #[must_use]
    pub const fn effective_range_px(self) -> f32 {
        match self {
            Self::CleavingStrike => 24.0,
            Self::RadialPulse => 40.0,
            Self::ChargingLunge => 48.0,
            Self::SupernovaBurst => 60.0,
        }
    }

    /// Pattern-specific damage multiplier.
    #[must_use]
    pub const fn damage_multiplier(self) -> f32 {
        match self {
            Self::CleavingStrike => 1.0,
            Self::RadialPulse => 1.15,
            Self::ChargingLunge => 1.25,
            Self::SupernovaBurst => 1.45,
        }
    }
}

/// ECS Component for the multi-phase Endboss.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BossController {
    pub boss_id: String,
    pub display_name: String,
    pub phase: BossPhase,
    /// Minimum character level required to pierce the boss's ancient barrier and realistically defeat it.
    pub recommended_min_level: u32,
    pub base_attack: i32,
    pub base_defense: i32,
    pub move_speed: f32,
    pub aggro_range_px: f32,
    pub attack_range_px: f32,
    pub base_attack_cooldown_secs: f32,
    pub attack_timer: f32,
    pub pattern_cursor: usize,
    pub xp_reward: u32,
    pub drop_table: Vec<String>,
}

impl Default for BossController {
    fn default() -> Self {
        Self::chrono_overlord()
    }
}

impl BossController {
    /// Creates the default Endboss (`Chrono-Lich Overlord`, realistically beatable from level 5+).
    #[must_use]
    pub fn chrono_overlord() -> Self {
        Self {
            boss_id: "entity.boss_chrono_overlord".to_string(),
            display_name: "Chrono-Lich Overlord".to_string(),
            phase: BossPhase::Phase1,
            recommended_min_level: 5,
            base_attack: 42,
            base_defense: 20,
            move_speed: 44.0,
            aggro_range_px: 96.0,
            attack_range_px: 28.0,
            base_attack_cooldown_secs: 1.1,
            attack_timer: 0.0,
            pattern_cursor: 0,
            xp_reward: 600,
            drop_table: vec!["weapon.laser_sword".to_string()],
        }
    }

    /// Updates `self.phase` based on the boss's current [`HealthPool`], returning `Some(new_phase)`
    /// if a phase transition occurred (phases only advance forward: `Phase1 -> Phase2 -> Phase3`).
    pub fn sync_phase(&mut self, health: &HealthPool) -> Option<BossPhase> {
        let target_phase = BossPhase::from_health_ratio(health.ratio());
        if target_phase > self.phase {
            self.phase = target_phase;
            self.pattern_cursor = 0;
            Some(target_phase)
        } else {
            None
        }
    }

    /// Returns the next [`BossAttackPattern`] in the current phase's rotation without advancing the cursor.
    #[must_use]
    pub fn current_attack_pattern(&self) -> BossAttackPattern {
        let rotation = self.phase.pattern_rotation();
        rotation[self.pattern_cursor % rotation.len()]
    }

    /// Advances the pattern cursor and returns the executed [`BossAttackPattern`].
    pub fn next_attack_pattern(&mut self) -> BossAttackPattern {
        let pattern = self.current_attack_pattern();
        let len = self.phase.pattern_rotation().len();
        self.pattern_cursor = (self.pattern_cursor + 1) % len;
        pattern
    }

    /// Computes damage dealt by the player to this boss, accounting for `base_defense`
    /// and the boss's level-gated barrier (`recommended_min_level`).
    #[must_use]
    pub fn compute_damage_taken(&self, player_level: u32, raw_player_damage: i32) -> i32 {
        let after_armor = (raw_player_damage - self.base_defense).max(1);
        if player_level < self.recommended_min_level {
            // Underleveled characters cannot pierce the boss's Chrono-Aegis barrier effectively
            let level_deficit = self.recommended_min_level - player_level;
            let divisor = (1 + level_deficit * 3) as i32;
            (after_armor / divisor).clamp(1, 3)
        } else {
            after_armor.max(5)
        }
    }

    /// Computes damage dealt by this boss to the player for a given [`BossAttackPattern`].
    #[must_use]
    pub fn compute_damage_dealt(
        &self,
        player_level: u32,
        player_defense: i32,
        pattern: BossAttackPattern,
    ) -> i32 {
        let scaled = (self.base_attack as f32)
            * self.phase.damage_multiplier()
            * pattern.damage_multiplier();
        let crushing_bonus = if player_level < self.recommended_min_level {
            ((self.recommended_min_level - player_level) as i32) * 8
        } else {
            0
        };
        (scaled.round() as i32 + crushing_bonus - player_defense).max(4)
    }
}

/// Summary report of a deterministic simulated duel between the player and the Endboss.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BossBattleReport {
    pub player_won: bool,
    pub ticks_elapsed: u32,
    pub player_remaining_hp: i32,
    pub boss_remaining_hp: i32,
    pub highest_phase_reached: BossPhase,
    pub patterns_executed: Vec<BossAttackPattern>,
}

/// Deterministically simulates a full combat encounter between a character with `progression`
/// and the `boss` (with `boss_max_hp`), verifying level-gating, weapon evolution impact,
/// active skills (`Dash`, `AreaAttack`), and multi-phase boss transitions.
#[must_use]
pub fn simulate_boss_encounter(
    progression: &CharacterProgression,
    registries: &crate::content::GameContentRegistries,
    skill_tree: &SkillTree,
    boss_template: &BossController,
    boss_max_hp: i32,
) -> BossBattleReport {
    let effective = progression.compute_effective_stats(registries, skill_tree);
    let mut player_stats = CombatStats::from_effective(effective);
    let mut boss = boss_template.clone();
    let mut boss_hp = HealthPool::new(boss_max_hp);

    let dash_skill = progression.unlocked_dash_skill(skill_tree);
    let aoe_skill = progression.unlocked_aoe_skill(skill_tree);

    let dt = 0.1_f32; // 100ms simulation step
    let max_ticks = 1200_u32; // up to 120 seconds of combat
    let mut highest_phase = boss.phase;
    let mut patterns_executed = Vec::new();

    for tick in 1..=max_ticks {
        player_stats.tick_timers(dt);
        boss.attack_timer = (boss.attack_timer - dt).max(0.0);

        // 1. Player uses Active AoE Skill if unlocked and off cooldown
        if let Some((_radius, mult, flat_bonus, cd)) = aoe_skill {
            if player_stats.aoe_cooldown <= 0.0 {
                let raw_aoe = ((player_stats.attack as f32) * mult).round() as i32 + flat_bonus;
                let dmg = boss.compute_damage_taken(progression.level, raw_aoe);
                boss_hp.apply_damage(dmg);
                player_stats.aoe_cooldown = cd;
                if let Some(new_phase) = boss.sync_phase(&boss_hp) {
                    highest_phase = highest_phase.max(new_phase);
                }
            }
        }

        // 2. Player uses Basic Weapon Attack when off cooldown
        if boss_hp.is_alive() && player_stats.basic_attack_cooldown <= 0.0 {
            let crit_mult = if player_stats.crit_chance >= 0.15 && tick.is_multiple_of(4) {
                1.5
            } else {
                1.0
            };
            let raw_hit = ((player_stats.attack as f32) * crit_mult).round() as i32;
            let dmg = boss.compute_damage_taken(progression.level, raw_hit);
            boss_hp.apply_damage(dmg);
            player_stats.basic_attack_cooldown =
                (1.0 / player_stats.attack_speed.max(0.2)).max(0.2);
            if let Some(new_phase) = boss.sync_phase(&boss_hp) {
                highest_phase = highest_phase.max(new_phase);
            }
        }

        if !boss_hp.is_alive() {
            return BossBattleReport {
                player_won: true,
                ticks_elapsed: tick,
                player_remaining_hp: player_stats.current_health,
                boss_remaining_hp: 0,
                highest_phase_reached: highest_phase,
                patterns_executed,
            };
        }

        // 3. Player proactively uses Dash before an imminent Boss attack if Dash is off cooldown
        if let Some((_dist, dash_cd, invuln_secs)) = dash_skill {
            if boss.attack_timer <= dt && player_stats.dash_cooldown <= 0.0 {
                player_stats.invulnerability_timer = invuln_secs;
                player_stats.dash_cooldown = dash_cd;
            }
        }

        // 4. Boss executes its current phase's attack pattern when attack_timer reaches 0
        if boss.attack_timer <= 0.0 {
            let pattern = boss.next_attack_pattern();
            patterns_executed.push(pattern);
            boss.attack_timer =
                (boss.base_attack_cooldown_secs * boss.phase.cooldown_multiplier()).max(0.25);

            if player_stats.invulnerability_timer <= 0.0 {
                let boss_dmg =
                    boss.compute_damage_dealt(progression.level, player_stats.defense, pattern);
                player_stats.current_health = (player_stats.current_health - boss_dmg).max(0);
            }
        }

        if !player_stats.is_alive() {
            return BossBattleReport {
                player_won: false,
                ticks_elapsed: tick,
                player_remaining_hp: 0,
                boss_remaining_hp: boss_hp.current,
                highest_phase_reached: highest_phase,
                patterns_executed,
            };
        }
    }

    BossBattleReport {
        player_won: boss_hp.current <= 0,
        ticks_elapsed: max_ticks,
        player_remaining_hp: player_stats.current_health,
        boss_remaining_hp: boss_hp.current,
        highest_phase_reached: highest_phase,
        patterns_executed,
    }
}

// ============================================================================
// 3. ECS Combat & Active Skill Events and Systems
// ============================================================================

/// Player combat or active skill command event.
#[derive(Event, Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayerCombatActionEvent {
    /// Performs a basic attack with the currently equipped weapon.
    BasicAttack,
    /// Executes the active `Dash` skill in the player's current facing direction (if unlocked).
    UseDash,
    /// Executes the active `AreaAttack` (AoE) skill around the player (if unlocked).
    UseAreaAttack,
}

/// Outcome events emitted by combat, active skills, enemy AI, and boss encounters.
#[derive(Event, Debug, Clone, PartialEq)]
pub enum CombatOutcomeEvent {
    PlayerAttackedTarget {
        target_id: String,
        damage: i32,
        remaining_hp: i32,
    },
    DashPerformed {
        from: Vec2,
        to: Vec2,
        invulnerability_secs: f32,
    },
    AreaAttackPerformed {
        center: Vec2,
        radius_px: f32,
        targets_hit: usize,
        total_damage: i32,
    },
    PlayerDamaged {
        attacker_id: String,
        damage: i32,
        remaining_hp: i32,
    },
    EnemyDefeated {
        entity_id: String,
        xp_awarded: u32,
        levels_gained: u32,
        is_boss: bool,
    },
    BossPhaseChanged {
        boss_id: String,
        new_phase: BossPhase,
    },
}

/// System that synchronizes [`CombatStats`] and [`PlayerMovement`] speed from [`CharacterProgression`]
/// and [`SkillTree`], and ticks combat cooldown timers.
pub fn sync_character_stats_system(
    input: Res<PlayerInputState>,
    registries: Res<crate::content::GameContentRegistries>,
    skill_tree: Res<SkillTree>,
    mut query: Query<
        (
            &CharacterProgression,
            &mut CombatStats,
            Option<&mut PlayerMovement>,
        ),
        With<HeroController>,
    >,
) {
    for (progression, mut stats, opt_movement) in &mut query {
        let effective = progression.compute_effective_stats(&registries, &skill_tree);
        stats.apply_effective_stats(effective, true);
        stats.tick_timers(input.delta_time);
        if let Some(mut movement) = opt_movement {
            movement.speed_px_per_sec = stats.speed;
        }
    }
}

/// System that processes [`PlayerCombatActionEvent`] (`BasicAttack`, `UseDash`, `UseAreaAttack`).
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn player_combat_and_skills_system(
    mut commands: Commands,
    mut action_events: EventReader<PlayerCombatActionEvent>,
    mut outcome_writer: EventWriter<CombatOutcomeEvent>,
    mut level_up_writer: EventWriter<LevelUpEvent>,
    registries: Res<crate::content::GameContentRegistries>,
    skill_tree: Res<SkillTree>,
    grid_config: Res<WorldGridConfig>,
    world_map: Res<WorldMap>,
    scene_manager: Res<SceneManager>,
    mut hero_query: Query<
        (
            &mut WorldPixelPos,
            &FacingDirection,
            &PlayerHitbox,
            &mut CharacterProgression,
            &mut CombatStats,
        ),
        With<HeroController>,
    >,
    mut enemies_query: Query<
        (
            Entity,
            &WorldPixelPos,
            &mut HealthPool,
            Option<&EnemyController>,
            Option<&mut BossController>,
            Option<&crate::interaction::OverworldPropEntity>,
            Option<&crate::interaction::InstanceSceneEntity>,
        ),
        Without<HeroController>,
    >,
) {
    let Some((mut hero_pos, facing, hitbox, mut progression, mut stats)) =
        hero_query.iter_mut().next()
    else {
        action_events.clear();
        return;
    };

    if !stats.is_alive() {
        action_events.clear();
        return;
    }

    let in_instance = scene_manager.is_instance();

    for action in action_events.read() {
        match action {
            PlayerCombatActionEvent::UseDash => {
                let Some((distance_px, cooldown_secs, invuln_secs)) =
                    progression.unlocked_dash_skill(&skill_tree)
                else {
                    continue;
                };
                if stats.dash_cooldown > 0.0 {
                    continue;
                }

                let start_vec = hero_pos.to_vec2();
                let dash_delta = facing.unit_vector() * distance_px;
                let current_world = WorldPos::from(*hero_pos);
                let (new_world, _applied) = match scene_manager.current_instance() {
                    Some(dungeon) => resolve_subtile_movement(
                        current_world,
                        hitbox,
                        dash_delta,
                        dungeon,
                        &grid_config,
                    ),
                    None => resolve_subtile_movement(
                        current_world,
                        hitbox,
                        dash_delta,
                        &*world_map,
                        &grid_config,
                    ),
                };

                *hero_pos = new_world.to_world_pixel_pos();
                stats.dash_cooldown = cooldown_secs;
                stats.invulnerability_timer = stats.invulnerability_timer.max(invuln_secs);

                outcome_writer.send(CombatOutcomeEvent::DashPerformed {
                    from: start_vec,
                    to: hero_pos.to_vec2(),
                    invulnerability_secs: invuln_secs,
                });
            }

            PlayerCombatActionEvent::UseAreaAttack => {
                let Some((radius_px, dmg_mult, flat_bonus, cooldown_secs)) =
                    progression.unlocked_aoe_skill(&skill_tree)
                else {
                    continue;
                };
                if stats.aoe_cooldown > 0.0 {
                    continue;
                }

                let center = hero_pos.to_vec2();
                let raw_aoe_damage = ((stats.attack as f32) * dmg_mult).round() as i32 + flat_bonus;
                let mut targets_hit = 0usize;
                let mut total_damage = 0i32;

                for (
                    entity,
                    target_pos,
                    mut hp,
                    opt_enemy,
                    mut opt_boss,
                    is_overworld_prop,
                    is_instance_prop,
                ) in &mut enemies_query
                {
                    if in_instance && is_overworld_prop.is_some() {
                        continue;
                    }
                    if !in_instance && is_instance_prop.is_some() {
                        continue;
                    }
                    if !hp.is_alive() {
                        continue;
                    }
                    let dist = target_pos.to_vec2().distance(center);
                    if dist > radius_px {
                        continue;
                    }

                    let (target_id, dmg, xp_reward, drops, is_boss) =
                        if let Some(ref mut boss) = opt_boss {
                            let d = boss.compute_damage_taken(progression.level, raw_aoe_damage);
                            (
                                boss.boss_id.clone(),
                                d,
                                boss.xp_reward,
                                boss.drop_table.clone(),
                                true,
                            )
                        } else if let Some(enemy) = opt_enemy {
                            let d = (raw_aoe_damage - enemy.defense).max(1);
                            (
                                enemy.entity_id.clone(),
                                d,
                                enemy.xp_reward,
                                enemy.drop_table.clone(),
                                false,
                            )
                        } else {
                            continue;
                        };

                    let dealt = hp.apply_damage(dmg);
                    targets_hit += 1;
                    total_damage += dealt;

                    if let Some(ref mut boss) = opt_boss {
                        if let Some(new_phase) = boss.sync_phase(&hp) {
                            outcome_writer.send(CombatOutcomeEvent::BossPhaseChanged {
                                boss_id: boss.boss_id.clone(),
                                new_phase,
                            });
                        }
                    }

                    if !hp.is_alive() {
                        handle_enemy_defeat(
                            &mut commands,
                            entity,
                            &target_id,
                            xp_reward,
                            &drops,
                            is_boss,
                            &mut progression,
                            &mut stats,
                            &registries,
                            &skill_tree,
                            &mut outcome_writer,
                            &mut level_up_writer,
                        );
                    }
                }

                stats.aoe_cooldown = cooldown_secs;
                outcome_writer.send(CombatOutcomeEvent::AreaAttackPerformed {
                    center,
                    radius_px,
                    targets_hit,
                    total_damage,
                });
            }

            PlayerCombatActionEvent::BasicAttack => {
                if stats.basic_attack_cooldown > 0.0 {
                    continue;
                }
                let center = hero_pos.to_vec2();
                let reach = stats.attack_range_px.max(18.0);

                for (
                    entity,
                    target_pos,
                    mut hp,
                    opt_enemy,
                    mut opt_boss,
                    is_overworld_prop,
                    is_instance_prop,
                ) in &mut enemies_query
                {
                    if in_instance && is_overworld_prop.is_some() {
                        continue;
                    }
                    if !in_instance && is_instance_prop.is_some() {
                        continue;
                    }
                    if !hp.is_alive() {
                        continue;
                    }
                    let dist = target_pos.to_vec2().distance(center);
                    if dist > reach {
                        continue;
                    }

                    let (target_id, dmg, xp_reward, drops, is_boss) =
                        if let Some(ref mut boss) = opt_boss {
                            let d = boss.compute_damage_taken(progression.level, stats.attack);
                            (
                                boss.boss_id.clone(),
                                d,
                                boss.xp_reward,
                                boss.drop_table.clone(),
                                true,
                            )
                        } else if let Some(enemy) = opt_enemy {
                            let d = (stats.attack - enemy.defense).max(1);
                            (
                                enemy.entity_id.clone(),
                                d,
                                enemy.xp_reward,
                                enemy.drop_table.clone(),
                                false,
                            )
                        } else {
                            continue;
                        };

                    hp.apply_damage(dmg);
                    outcome_writer.send(CombatOutcomeEvent::PlayerAttackedTarget {
                        target_id: target_id.clone(),
                        damage: dmg,
                        remaining_hp: hp.current,
                    });

                    if let Some(ref mut boss) = opt_boss {
                        if let Some(new_phase) = boss.sync_phase(&hp) {
                            outcome_writer.send(CombatOutcomeEvent::BossPhaseChanged {
                                boss_id: boss.boss_id.clone(),
                                new_phase,
                            });
                        }
                    }

                    if !hp.is_alive() {
                        handle_enemy_defeat(
                            &mut commands,
                            entity,
                            &target_id,
                            xp_reward,
                            &drops,
                            is_boss,
                            &mut progression,
                            &mut stats,
                            &registries,
                            &skill_tree,
                            &mut outcome_writer,
                            &mut level_up_writer,
                        );
                    }
                    break;
                }

                stats.basic_attack_cooldown = (1.0 / stats.attack_speed.max(0.2)).max(0.15);
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn handle_enemy_defeat(
    commands: &mut Commands,
    entity: Entity,
    target_id: &str,
    xp_reward: u32,
    drops: &[String],
    is_boss: bool,
    progression: &mut CharacterProgression,
    stats: &mut CombatStats,
    registries: &crate::content::GameContentRegistries,
    skill_tree: &SkillTree,
    outcome_writer: &mut EventWriter<CombatOutcomeEvent>,
    level_up_writer: &mut EventWriter<LevelUpEvent>,
) {
    let old_level = progression.level;
    let levels_gained = progression.gain_xp(xp_reward);
    for item in drops {
        if !progression.inventory.contains(item) {
            progression.inventory.push(item.clone());
        }
    }

    if levels_gained > 0 {
        let effective = progression.compute_effective_stats(registries, skill_tree);
        stats.apply_effective_stats(effective, true);
        level_up_writer.send(LevelUpEvent {
            old_level,
            new_level: progression.level,
            attribute_points_gained: levels_gained * ATTRIBUTE_POINTS_PER_LEVEL,
            skill_points_gained: levels_gained * SKILL_POINTS_PER_LEVEL,
        });
    }

    outcome_writer.send(CombatOutcomeEvent::EnemyDefeated {
        entity_id: target_id.to_string(),
        xp_awarded: xp_reward,
        levels_gained,
        is_boss,
    });

    commands.entity(entity).despawn();
}

/// System executing the Standard Enemy Finite State Machine (`Patrol` -> `Chase` -> `Attack`)
/// and Endboss multi-phase AI (`Phase1` -> `Phase2` -> `Phase3`).
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn enemy_and_boss_ai_system(
    input: Res<PlayerInputState>,
    grid_config: Res<WorldGridConfig>,
    world_map: Res<WorldMap>,
    scene_manager: Res<SceneManager>,
    mut outcome_writer: EventWriter<CombatOutcomeEvent>,
    mut hero_query: Query<
        (&WorldPixelPos, &CharacterProgression, &mut CombatStats),
        With<HeroController>,
    >,
    mut enemy_query: Query<
        (
            &mut WorldPixelPos,
            &mut EnemyController,
            &HealthPool,
            Option<&crate::interaction::OverworldPropEntity>,
            Option<&crate::interaction::InstanceSceneEntity>,
        ),
        (Without<HeroController>, Without<BossController>),
    >,
    mut boss_query: Query<
        (
            &mut WorldPixelPos,
            &mut BossController,
            &HealthPool,
            Option<&crate::interaction::OverworldPropEntity>,
            Option<&crate::interaction::InstanceSceneEntity>,
        ),
        (Without<HeroController>, Without<EnemyController>),
    >,
) {
    let Some((hero_pos_px, progression, mut hero_stats)) = hero_query.iter_mut().next() else {
        return;
    };
    if !hero_stats.is_alive() {
        return;
    }

    let dt = input.delta_time.max(0.0);
    let hero_pos = hero_pos_px.to_vec2();
    let entity_hitbox = PlayerHitbox::default();
    let in_instance = scene_manager.is_instance();

    // 1. Update Standard Enemies (Patrol / Chase / Attack)
    for (mut pos, mut enemy, hp, is_overworld_prop, is_instance_prop) in &mut enemy_query {
        if in_instance && is_overworld_prop.is_some() {
            continue;
        }
        if !in_instance && is_instance_prop.is_some() {
            continue;
        }
        if !hp.is_alive() {
            continue;
        }
        enemy.attack_timer = (enemy.attack_timer - dt).max(0.0);

        let current_pos = pos.to_vec2();
        let dist_to_player = current_pos.distance(hero_pos);

        enemy.ai_state = evaluate_enemy_ai_state(
            enemy.ai_state,
            dist_to_player,
            enemy.aggro_range_px,
            enemy.attack_range_px,
            enemy.leash_range_px,
        );

        match enemy.ai_state {
            EnemyAiState::Patrol => {
                if !enemy.patrol_waypoints.is_empty() {
                    let idx = enemy.current_waypoint_index % enemy.patrol_waypoints.len();
                    let target_wp = enemy.patrol_waypoints[idx];
                    let to_wp = target_wp - current_pos;
                    if to_wp.length() <= 2.0 {
                        enemy.current_waypoint_index = (idx + 1) % enemy.patrol_waypoints.len();
                    } else {
                        let step = compute_normalized_movement_delta(to_wp, enemy.patrol_speed, dt);
                        let (new_pos, _) = match scene_manager.current_instance() {
                            Some(dungeon) => resolve_subtile_movement(
                                WorldPos::from(*pos),
                                &entity_hitbox,
                                step,
                                dungeon,
                                &grid_config,
                            ),
                            None => resolve_subtile_movement(
                                WorldPos::from(*pos),
                                &entity_hitbox,
                                step,
                                &*world_map,
                                &grid_config,
                            ),
                        };
                        *pos = new_pos.to_world_pixel_pos();
                    }
                }
            }
            EnemyAiState::Chase => {
                let to_player = hero_pos - current_pos;
                let step = compute_normalized_movement_delta(to_player, enemy.chase_speed, dt);
                let (new_pos, _) = match scene_manager.current_instance() {
                    Some(dungeon) => resolve_subtile_movement(
                        WorldPos::from(*pos),
                        &entity_hitbox,
                        step,
                        dungeon,
                        &grid_config,
                    ),
                    None => resolve_subtile_movement(
                        WorldPos::from(*pos),
                        &entity_hitbox,
                        step,
                        &*world_map,
                        &grid_config,
                    ),
                };
                *pos = new_pos.to_world_pixel_pos();
            }
            EnemyAiState::Attack => {
                if enemy.attack_timer <= 0.0 {
                    enemy.attack_timer = enemy.attack_cooldown_secs.max(0.2);
                    if hero_stats.invulnerability_timer <= 0.0 {
                        let dmg = (enemy.attack_damage - hero_stats.defense).max(1);
                        hero_stats.current_health = (hero_stats.current_health - dmg).max(0);
                        outcome_writer.send(CombatOutcomeEvent::PlayerDamaged {
                            attacker_id: enemy.entity_id.clone(),
                            damage: dmg,
                            remaining_hp: hero_stats.current_health,
                        });
                    }
                }
            }
        }
    }

    // 2. Update Multi-Phase Endboss
    for (mut pos, mut boss, hp, is_overworld_prop, is_instance_prop) in &mut boss_query {
        if in_instance && is_overworld_prop.is_some() {
            continue;
        }
        if !in_instance && is_instance_prop.is_some() {
            continue;
        }
        if !hp.is_alive() {
            continue;
        }
        if let Some(new_phase) = boss.sync_phase(hp) {
            outcome_writer.send(CombatOutcomeEvent::BossPhaseChanged {
                boss_id: boss.boss_id.clone(),
                new_phase,
            });
        }

        boss.attack_timer = (boss.attack_timer - dt).max(0.0);
        let current_pos = pos.to_vec2();
        let dist_to_player = current_pos.distance(hero_pos);

        if dist_to_player > boss.aggro_range_px {
            continue;
        }

        let upcoming_pattern = boss.current_attack_pattern();
        let pattern_reach = upcoming_pattern
            .effective_range_px()
            .max(boss.attack_range_px);

        if dist_to_player > pattern_reach {
            let speed = boss.move_speed * boss.phase.speed_multiplier();
            let step = compute_normalized_movement_delta(hero_pos - current_pos, speed, dt);
            let (new_pos, _) = match scene_manager.current_instance() {
                Some(dungeon) => resolve_subtile_movement(
                    WorldPos::from(*pos),
                    &entity_hitbox,
                    step,
                    dungeon,
                    &grid_config,
                ),
                None => resolve_subtile_movement(
                    WorldPos::from(*pos),
                    &entity_hitbox,
                    step,
                    &*world_map,
                    &grid_config,
                ),
            };
            *pos = new_pos.to_world_pixel_pos();
        } else if boss.attack_timer <= 0.0 {
            let executed_pattern = boss.next_attack_pattern();
            boss.attack_timer =
                (boss.base_attack_cooldown_secs * boss.phase.cooldown_multiplier()).max(0.25);

            // If pattern is ChargingLunge, close remaining gap toward the player
            if executed_pattern == BossAttackPattern::ChargingLunge && dist_to_player > 12.0 {
                let lunge_step = (hero_pos - current_pos).normalize_or_zero() * 12.0;
                let (new_pos, _) = resolve_subtile_movement(
                    WorldPos::from(*pos),
                    &entity_hitbox,
                    lunge_step,
                    &*world_map,
                    &grid_config,
                );
                *pos = new_pos.to_world_pixel_pos();
            }

            if hero_stats.invulnerability_timer <= 0.0 {
                let dmg = boss.compute_damage_dealt(
                    progression.level,
                    hero_stats.defense,
                    executed_pattern,
                );
                hero_stats.current_health = (hero_stats.current_health - dmg).max(0);
                outcome_writer.send(CombatOutcomeEvent::PlayerDamaged {
                    attacker_id: boss.boss_id.clone(),
                    damage: dmg,
                    remaining_hp: hero_stats.current_health,
                });
            }
        }
    }
}
