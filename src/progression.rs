//! Character Progression, Data-Driven Class & Weapon Evolution, XP/Leveling,
//! Attribute Allocation, and Branching Skilltree (Phase 4).
//!
//! Provides:
//! - [`CharacterProgression`]: Tracks class, level, XP, unspent attribute/skill points,
//!   unlocked skill nodes, equipped weapon, and inventory.
//! - Weapon evolution (`evolve_weapon`) from simple starter weapons (e.g. `weapon.rusty_dagger`)
//!   through intermediate/advanced tiers to Endgame/Sci-Fi weapons (e.g. `weapon.laser_sword`).
//! - [`SkillTree`] & [`SkillNode`]: Branching skilltree supporting both passive stat bonuses
//!   (Attack, Speed, Defense, Max HP, Crit) and active combat skills ([`ActiveSkillKind::Dash`],
//!   [`ActiveSkillKind::AreaAttack`]).
//! - [`CombatStats`] & [`EffectiveStats`]: Deterministic stat aggregation from class, level,
//!   attributes, skilltree, and equipped weapon.

use std::collections::{BTreeMap, BTreeSet};

use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::content::{ContentDefinition, ContentError, GameContentRegistries, WeaponDefinition};

/// Attribute points granted upon each level-up.
pub const ATTRIBUTE_POINTS_PER_LEVEL: u32 = 3;

/// Skilltree points granted upon each level-up.
pub const SKILL_POINTS_PER_LEVEL: u32 = 1;

/// Base XP multiplier per level (`xp_required = level * BASE_XP_PER_LEVEL`).
pub const BASE_XP_PER_LEVEL: u32 = 100;

/// Errors that can occur during character progression, leveling, skill unlocking, or weapon evolution.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ProgressionError {
    #[error("Unknown character class ID '{0}'")]
    UnknownClass(String),
    #[error("Unknown weapon ID '{0}'")]
    UnknownWeapon(String),
    #[error("Unknown skill node ID '{0}'")]
    UnknownSkill(String),
    #[error("Weapon '{0}' has no further evolution stages")]
    NoFurtherWeaponEvolution(String),
    #[error("Weapon '{current_weapon}' cannot evolve directly into '{target_weapon}'")]
    InvalidEvolutionTarget {
        current_weapon: String,
        target_weapon: String,
    },
    #[error(
        "Character level {current_level} is too low for weapon '{weapon_id}' (requires level {required_level})"
    )]
    InsufficientLevelForWeapon {
        weapon_id: String,
        required_level: u32,
        current_level: u32,
    },
    #[error("Not enough attribute points (requested {requested}, available {available})")]
    InsufficientAttributePoints { requested: u32, available: u32 },
    #[error("Not enough skill points to unlock '{skill_id}' (cost {cost}, available {available})")]
    InsufficientSkillPoints {
        skill_id: String,
        cost: u32,
        available: u32,
    },
    #[error(
        "Character level {current_level} is too low to unlock skill '{skill_id}' (requires level {required_level})"
    )]
    InsufficientLevelForSkill {
        skill_id: String,
        required_level: u32,
        current_level: u32,
    },
    #[error("Skill node '{0}' is already unlocked")]
    SkillAlreadyUnlocked(String),
    #[error("Missing prerequisite skill node(s) for '{skill_id}': {missing:?}")]
    MissingSkillPrerequisites {
        skill_id: String,
        missing: Vec<String>,
    },
}

/// Primary character attributes that can be upgraded using attribute points gained on level-up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AttributeStat {
    /// Increases physical/energy attack damage (`+3` Attack per point).
    Attack,
    /// Increases movement speed (`+2.5` px/s) and critical strike chance (`+1%` per point).
    Speed,
    /// Increases armor mitigation (`+2` Defense) and maximum health (`+8` HP per point).
    Defense,
    /// Increases maximum health (`+15` HP) and toughness (`+1` Defense per point).
    Vitality,
}

/// Allocated attribute points on a character.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CharacterAttributes {
    pub attack: u32,
    pub speed: u32,
    pub defense: u32,
    pub vitality: u32,
}

impl CharacterAttributes {
    /// Total attribute points spent across all stats.
    #[must_use]
    pub const fn total_spent(&self) -> u32 {
        self.attack + self.speed + self.defense + self.vitality
    }

    /// Adds `points` to the given [`AttributeStat`].
    pub fn add(&mut self, stat: AttributeStat, points: u32) {
        match stat {
            AttributeStat::Attack => self.attack = self.attack.saturating_add(points),
            AttributeStat::Speed => self.speed = self.speed.saturating_add(points),
            AttributeStat::Defense => self.defense = self.defense.saturating_add(points),
            AttributeStat::Vitality => self.vitality = self.vitality.saturating_add(points),
        }
    }
}

/// Thematic branch in the character skilltree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SkillBranch {
    /// Offensive power, critical strikes, and Area-of-Effect nova attacks.
    Offense,
    /// Movement speed, evasion, and directional Dash ability.
    Mobility,
    /// Armor defense, maximum health, and bastion shockwave skills.
    Defense,
}

/// Active combat/mobility skills unlockable in the [`SkillTree`].
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum ActiveSkillKind {
    /// Rapid directional dash that propels the player forward and grants brief invulnerability.
    Dash {
        distance_px: f32,
        cooldown_secs: f32,
        invulnerability_secs: f32,
    },
    /// Radial area-of-effect (AoE) attack damaging all hostile targets within `radius_px`.
    AreaAttack {
        radius_px: f32,
        damage_multiplier: f32,
        flat_damage_bonus: i32,
        cooldown_secs: f32,
    },
}

/// Effect granted by unlocking a [`SkillNode`] in the [`SkillTree`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SkillEffect {
    /// Permanent passive stat increases.
    PassiveStats {
        attack_bonus: i32,
        speed_bonus: f32,
        defense_bonus: i32,
        max_health_bonus: i32,
        crit_chance_bonus: f32,
    },
    /// Unlocks an executable [`ActiveSkillKind`].
    ActiveSkill(ActiveSkillKind),
}

/// A single node in the branching [`SkillTree`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SkillNode {
    pub id: String,
    pub display_name: String,
    pub description: String,
    pub branch: SkillBranch,
    pub cost_points: u32,
    pub required_level: u32,
    /// Parent skill node IDs required before this node can be unlocked.
    #[serde(default)]
    pub prerequisites: Vec<String>,
    /// If `true`, unlocking ANY single prerequisite in `prerequisites` suffices (diamond merge node).
    /// If `false`, ALL listed `prerequisites` must be unlocked first.
    #[serde(default)]
    pub requires_any_prerequisite: bool,
    pub effect: SkillEffect,
}

impl ContentDefinition for SkillNode {
    const CONTENT_KIND: &'static str = "Skill";

    fn id(&self) -> &str {
        &self.id
    }

    fn display_name(&self) -> &str {
        &self.display_name
    }

    fn validate(&self) -> Result<(), ContentError> {
        if self.id.trim().is_empty() {
            return Err(ContentError::EmptyId {
                content_kind: Self::CONTENT_KIND,
            });
        }
        if self.display_name.trim().is_empty() {
            return Err(ContentError::EmptyDisplayName {
                content_kind: Self::CONTENT_KIND,
                id: self.id.clone(),
            });
        }
        if self.cost_points == 0 {
            return Err(ContentError::ValidationFailed {
                content_kind: Self::CONTENT_KIND,
                id: self.id.clone(),
                reason: "cost_points must be >= 1".to_string(),
            });
        }
        if self.required_level == 0 {
            return Err(ContentError::ValidationFailed {
                content_kind: Self::CONTENT_KIND,
                id: self.id.clone(),
                reason: "required_level must be >= 1".to_string(),
            });
        }
        Ok(())
    }
}

/// Aggregated passive stat bonuses computed from all unlocked nodes in a [`SkillTree`].
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct PassiveSkillBonuses {
    pub attack_bonus: i32,
    pub speed_bonus: f32,
    pub defense_bonus: i32,
    pub max_health_bonus: i32,
    pub crit_chance_bonus: f32,
}

/// Bevy Resource holding the branching character [`SkillTree`].
#[derive(Resource, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SkillTree {
    nodes: BTreeMap<String, SkillNode>,
}

impl Default for SkillTree {
    fn default() -> Self {
        Self::default_branching_tree()
    }
}

impl SkillTree {
    /// Creates an empty [`SkillTree`].
    #[must_use]
    pub fn new() -> Self {
        Self {
            nodes: BTreeMap::new(),
        }
    }

    /// Builds the default 3-branch skilltree (`Offense`, `Mobility`, `Defense`) with
    /// passive stat nodes, branching sub-paths, active skills (`Dash`, `AreaAttack`), and capstones.
    #[must_use]
    pub fn default_branching_tree() -> Self {
        let mut tree = Self::new();
        let default_nodes = [
            // ================================================================
            // Branch 1: Offense (Attack, Crit & AreaAttack)
            // ================================================================
            SkillNode {
                id: "skill.offense_root".to_string(),
                display_name: "Klingenschärfe I".to_string(),
                description: "Erhöht den Basis-Angriffsschaden um +6.".to_string(),
                branch: SkillBranch::Offense,
                cost_points: 1,
                required_level: 1,
                prerequisites: vec![],
                requires_any_prerequisite: false,
                effect: SkillEffect::PassiveStats {
                    attack_bonus: 6,
                    speed_bonus: 0.0,
                    defense_bonus: 0,
                    max_health_bonus: 0,
                    crit_chance_bonus: 0.0,
                },
            },
            SkillNode {
                id: "skill.offense_crit".to_string(),
                display_name: "Präzisionsschnitt II".to_string(),
                description: "Verzweigter Pfad A: +10 Angriff und +8% kritische Trefferchance."
                    .to_string(),
                branch: SkillBranch::Offense,
                cost_points: 1,
                required_level: 2,
                prerequisites: vec!["skill.offense_root".to_string()],
                requires_any_prerequisite: false,
                effect: SkillEffect::PassiveStats {
                    attack_bonus: 10,
                    speed_bonus: 0.0,
                    defense_bonus: 0,
                    max_health_bonus: 0,
                    crit_chance_bonus: 0.08,
                },
            },
            SkillNode {
                id: "skill.active_aoe_strike".to_string(),
                display_name: "Nova-Flächenangriff".to_string(),
                description:
                    "Verzweigter Pfad B: Schaltet den aktiven Nova-Flächenangriff (AoE) frei."
                        .to_string(),
                branch: SkillBranch::Offense,
                cost_points: 1,
                required_level: 2,
                prerequisites: vec!["skill.offense_root".to_string()],
                requires_any_prerequisite: false,
                effect: SkillEffect::ActiveSkill(ActiveSkillKind::AreaAttack {
                    radius_px: 48.0,
                    damage_multiplier: 1.6,
                    flat_damage_bonus: 18,
                    cooldown_secs: 2.5,
                }),
            },
            SkillNode {
                id: "skill.offense_capstone".to_string(),
                display_name: "Laserkern-Überladung".to_string(),
                description: "Endstufe Angriff: +18 Angriff, +4 Speed und +10% Crit.".to_string(),
                branch: SkillBranch::Offense,
                cost_points: 1,
                required_level: 4,
                prerequisites: vec![
                    "skill.offense_crit".to_string(),
                    "skill.active_aoe_strike".to_string(),
                ],
                requires_any_prerequisite: true,
                effect: SkillEffect::PassiveStats {
                    attack_bonus: 18,
                    speed_bonus: 4.0,
                    defense_bonus: 0,
                    max_health_bonus: 0,
                    crit_chance_bonus: 0.10,
                },
            },
            // ================================================================
            // Branch 2: Mobility (Speed, Evasion & Dash)
            // ================================================================
            SkillNode {
                id: "skill.mobility_root".to_string(),
                display_name: "Windschritt I".to_string(),
                description: "Erhöht die Bewegungsgeschwindigkeit um +8 px/s.".to_string(),
                branch: SkillBranch::Mobility,
                cost_points: 1,
                required_level: 1,
                prerequisites: vec![],
                requires_any_prerequisite: false,
                effect: SkillEffect::PassiveStats {
                    attack_bonus: 0,
                    speed_bonus: 8.0,
                    defense_bonus: 0,
                    max_health_bonus: 0,
                    crit_chance_bonus: 0.0,
                },
            },
            SkillNode {
                id: "skill.active_dash".to_string(),
                display_name: "Phantom-Dash".to_string(),
                description:
                    "Verzweigter Pfad A: Schaltet den aktiven Dash mit kurzer Unverwundbarkeit frei."
                        .to_string(),
                branch: SkillBranch::Mobility,
                cost_points: 1,
                required_level: 2,
                prerequisites: vec!["skill.mobility_root".to_string()],
                requires_any_prerequisite: false,
                effect: SkillEffect::ActiveSkill(ActiveSkillKind::Dash {
                    distance_px: 40.0,
                    cooldown_secs: 1.5,
                    invulnerability_secs: 0.35,
                }),
            },
            SkillNode {
                id: "skill.mobility_evasion".to_string(),
                display_name: "Reflex-Beschleunigung II".to_string(),
                description: "Verzweigter Pfad B: +12 Speed und +3 Verteidigung.".to_string(),
                branch: SkillBranch::Mobility,
                cost_points: 1,
                required_level: 2,
                prerequisites: vec!["skill.mobility_root".to_string()],
                requires_any_prerequisite: false,
                effect: SkillEffect::PassiveStats {
                    attack_bonus: 0,
                    speed_bonus: 12.0,
                    defense_bonus: 3,
                    max_health_bonus: 0,
                    crit_chance_bonus: 0.04,
                },
            },
            SkillNode {
                id: "skill.mobility_capstone".to_string(),
                display_name: "Lichtgeschwindigkeit".to_string(),
                description: "Endstufe Mobilität: +16 Speed und +8 Angriff.".to_string(),
                branch: SkillBranch::Mobility,
                cost_points: 1,
                required_level: 4,
                prerequisites: vec![
                    "skill.active_dash".to_string(),
                    "skill.mobility_evasion".to_string(),
                ],
                requires_any_prerequisite: true,
                effect: SkillEffect::PassiveStats {
                    attack_bonus: 8,
                    speed_bonus: 16.0,
                    defense_bonus: 2,
                    max_health_bonus: 0,
                    crit_chance_bonus: 0.05,
                },
            },
            // ================================================================
            // Branch 3: Defense (Defense, Max HP & Bastion Shockwave)
            // ================================================================
            SkillNode {
                id: "skill.defense_root".to_string(),
                display_name: "Eisenhaut I".to_string(),
                description: "Erhöht Verteidigung um +5 und maximale HP um +25.".to_string(),
                branch: SkillBranch::Defense,
                cost_points: 1,
                required_level: 1,
                prerequisites: vec![],
                requires_any_prerequisite: false,
                effect: SkillEffect::PassiveStats {
                    attack_bonus: 0,
                    speed_bonus: 0.0,
                    defense_bonus: 5,
                    max_health_bonus: 25,
                    crit_chance_bonus: 0.0,
                },
            },
            SkillNode {
                id: "skill.defense_fortress".to_string(),
                display_name: "Bastionspanzer II".to_string(),
                description: "Verzweigter Pfad A: +9 Verteidigung und +45 maximale HP.".to_string(),
                branch: SkillBranch::Defense,
                cost_points: 1,
                required_level: 2,
                prerequisites: vec!["skill.defense_root".to_string()],
                requires_any_prerequisite: false,
                effect: SkillEffect::PassiveStats {
                    attack_bonus: 0,
                    speed_bonus: 0.0,
                    defense_bonus: 9,
                    max_health_bonus: 45,
                    crit_chance_bonus: 0.0,
                },
            },
            SkillNode {
                id: "skill.active_shockwave".to_string(),
                display_name: "Aegis-Schockwelle".to_string(),
                description:
                    "Verzweigter Pfad B: Schaltet eine defensive Flächen-Schockwelle frei."
                        .to_string(),
                branch: SkillBranch::Defense,
                cost_points: 1,
                required_level: 3,
                prerequisites: vec!["skill.defense_root".to_string()],
                requires_any_prerequisite: false,
                effect: SkillEffect::ActiveSkill(ActiveSkillKind::AreaAttack {
                    radius_px: 56.0,
                    damage_multiplier: 1.4,
                    flat_damage_bonus: 22,
                    cooldown_secs: 3.0,
                }),
            },
            SkillNode {
                id: "skill.defense_capstone".to_string(),
                display_name: "Unerschütterlicher Wächter".to_string(),
                description: "Endstufe Verteidigung: +14 Verteidigung, +80 HP und +6 Angriff."
                    .to_string(),
                branch: SkillBranch::Defense,
                cost_points: 1,
                required_level: 4,
                prerequisites: vec![
                    "skill.defense_fortress".to_string(),
                    "skill.active_shockwave".to_string(),
                ],
                requires_any_prerequisite: true,
                effect: SkillEffect::PassiveStats {
                    attack_bonus: 6,
                    speed_bonus: 0.0,
                    defense_bonus: 14,
                    max_health_bonus: 80,
                    crit_chance_bonus: 0.0,
                },
            },
        ];

        for node in default_nodes {
            tree.register_node(node)
                .expect("Default branching skilltree nodes must be valid");
        }
        tree
    }

    /// Registers or overrides a [`SkillNode`] in the skilltree.
    pub fn register_node(&mut self, node: SkillNode) -> Result<(), ContentError> {
        node.validate()?;
        self.nodes.insert(node.id.clone(), node);
        Ok(())
    }

    /// Retrieves a [`SkillNode`] by ID.
    #[must_use]
    pub fn get(&self, skill_id: &str) -> Option<&SkillNode> {
        self.nodes.get(skill_id)
    }

    /// Total number of nodes in the skilltree.
    #[must_use]
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Returns `true` if the skilltree has no nodes.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Iterates over all skill nodes in deterministic ID order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &SkillNode)> {
        self.nodes.iter().map(|(k, v)| (k.as_str(), v))
    }

    /// Returns all nodes belonging to a specific [`SkillBranch`].
    #[must_use]
    pub fn nodes_in_branch(&self, branch: SkillBranch) -> Vec<&SkillNode> {
        self.nodes.values().filter(|n| n.branch == branch).collect()
    }
}

/// Computed effective stats of a character combining Class, Level, Attributes, SkillTree, and Weapon.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct EffectiveStats {
    pub max_health: i32,
    pub max_mana: i32,
    pub attack: i32,
    pub defense: i32,
    pub speed: f32,
    pub crit_chance: f32,
    pub attack_range_px: f32,
    pub attack_speed: f32,
}

/// ECS Component tracking live combat health, stats, and active skill cooldowns for the player.
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CombatStats {
    pub max_health: i32,
    pub current_health: i32,
    pub max_mana: i32,
    pub current_mana: i32,
    pub attack: i32,
    pub defense: i32,
    pub speed: f32,
    pub crit_chance: f32,
    pub attack_range_px: f32,
    pub attack_speed: f32,
    /// Remaining invulnerability frames/seconds (e.g. during Dash or post-hit grace).
    pub invulnerability_timer: f32,
    /// Cooldown timer before the next basic weapon attack.
    pub basic_attack_cooldown: f32,
    /// Cooldown timer before the next active Dash skill.
    pub dash_cooldown: f32,
    /// Cooldown timer before the next active AreaAttack (AoE) skill.
    pub aoe_cooldown: f32,
}

impl Default for CombatStats {
    fn default() -> Self {
        Self {
            max_health: 100,
            current_health: 100,
            max_mana: 40,
            current_mana: 40,
            attack: 16,
            defense: 4,
            speed: 64.0,
            crit_chance: 0.05,
            attack_range_px: 18.0,
            attack_speed: 1.4,
            invulnerability_timer: 0.0,
            basic_attack_cooldown: 0.0,
            dash_cooldown: 0.0,
            aoe_cooldown: 0.0,
        }
    }
}

impl CombatStats {
    /// Creates a new [`CombatStats`] initialized at full health and mana from [`EffectiveStats`].
    #[must_use]
    pub fn from_effective(effective: EffectiveStats) -> Self {
        Self {
            max_health: effective.max_health,
            current_health: effective.max_health,
            max_mana: effective.max_mana,
            current_mana: effective.max_mana,
            attack: effective.attack,
            defense: effective.defense,
            speed: effective.speed,
            crit_chance: effective.crit_chance,
            attack_range_px: effective.attack_range_px,
            attack_speed: effective.attack_speed,
            invulnerability_timer: 0.0,
            basic_attack_cooldown: 0.0,
            dash_cooldown: 0.0,
            aoe_cooldown: 0.0,
        }
    }

    /// Synchronizes derived stat maximums from `effective` while preserving current health ratio/clamping,
    /// or optionally healing by any `max_health` increase.
    pub fn apply_effective_stats(&mut self, effective: EffectiveStats, heal_delta: bool) {
        let hp_increase = (effective.max_health - self.max_health).max(0);
        self.max_health = effective.max_health;
        self.max_mana = effective.max_mana;
        self.attack = effective.attack;
        self.defense = effective.defense;
        self.speed = effective.speed;
        self.crit_chance = effective.crit_chance;
        self.attack_range_px = effective.attack_range_px;
        self.attack_speed = effective.attack_speed;

        if heal_delta && hp_increase > 0 && self.current_health > 0 {
            self.current_health = (self.current_health + hp_increase).min(self.max_health);
        } else {
            self.current_health = self.current_health.min(self.max_health);
        }
    }

    ///Ticks down all active cooldown and invulnerability timers by `dt` seconds.
    pub fn tick_timers(&mut self, dt: f32) {
        if dt <= 0.0 || !dt.is_finite() {
            return;
        }
        self.invulnerability_timer = (self.invulnerability_timer - dt).max(0.0);
        self.basic_attack_cooldown = (self.basic_attack_cooldown - dt).max(0.0);
        self.dash_cooldown = (self.dash_cooldown - dt).max(0.0);
        self.aoe_cooldown = (self.aoe_cooldown - dt).max(0.0);
    }

    /// Returns `true` if the character is alive (`current_health > 0`).
    #[must_use]
    pub const fn is_alive(&self) -> bool {
        self.current_health > 0
    }
}

/// ECS Component (and serializable progression state) managing the player's class, level, XP,
/// attribute points, unlocked skilltree nodes, and weapon evolution.
#[derive(Component, Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CharacterProgression {
    pub class_id: String,
    pub level: u32,
    pub current_xp: u32,
    pub total_xp_earned: u64,
    pub unspent_attribute_points: u32,
    pub unspent_skill_points: u32,
    pub attributes: CharacterAttributes,
    pub unlocked_skills: BTreeSet<String>,
    pub equipped_weapon_id: String,
    pub inventory: Vec<String>,
}

impl Default for CharacterProgression {
    fn default() -> Self {
        Self {
            class_id: "class.blade_vanguard".to_string(),
            level: 1,
            current_xp: 0,
            total_xp_earned: 0,
            unspent_attribute_points: 0,
            unspent_skill_points: 1, // 1 starter skill point at level 1
            attributes: CharacterAttributes::default(),
            unlocked_skills: BTreeSet::new(),
            equipped_weapon_id: "weapon.rusty_dagger".to_string(),
            inventory: vec![
                "weapon.rusty_dagger".to_string(),
                "item.berry_potion".to_string(),
            ],
        }
    }
}

impl CharacterProgression {
    /// Creates a new [`CharacterProgression`] initialized from a registered `ClassDefinition` ID.
    pub fn from_class(
        class_id: &str,
        registries: &GameContentRegistries,
    ) -> Result<Self, ProgressionError> {
        let Some(class_def) = registries.classes.get(class_id) else {
            return Err(ProgressionError::UnknownClass(class_id.to_string()));
        };

        // Determine starting weapon from class's starting_items (or fallback to rusty_dagger)
        let mut equipped_weapon_id = "weapon.rusty_dagger".to_string();
        for item_id in &class_def.starting_items {
            if let Some(resolved) = resolve_weapon_id(item_id, registries) {
                equipped_weapon_id = resolved;
                break;
            }
        }

        let mut inventory = class_def.starting_items.clone();
        if !inventory.contains(&equipped_weapon_id) {
            inventory.push(equipped_weapon_id.clone());
        }

        Ok(Self {
            class_id: class_def.id.clone(),
            level: 1,
            current_xp: 0,
            total_xp_earned: 0,
            unspent_attribute_points: 0,
            unspent_skill_points: 1,
            attributes: CharacterAttributes::default(),
            unlocked_skills: BTreeSet::new(),
            equipped_weapon_id,
            inventory,
        })
    }

    /// Switches or re-initializes the character's class from `registries`.
    pub fn change_class(
        &mut self,
        class_id: &str,
        registries: &GameContentRegistries,
    ) -> Result<(), ProgressionError> {
        let Some(class_def) = registries.classes.get(class_id) else {
            return Err(ProgressionError::UnknownClass(class_id.to_string()));
        };
        self.class_id = class_def.id.clone();
        for item_id in &class_def.starting_items {
            if !self.inventory.contains(item_id) {
                self.inventory.push(item_id.clone());
            }
            if let Some(resolved) = resolve_weapon_id(item_id, registries) {
                if self.level == 1 {
                    self.equipped_weapon_id = resolved;
                }
            }
        }
        Ok(())
    }

    /// Pure function returning the XP threshold required to advance from `level` to `level + 1`.
    #[must_use]
    pub const fn xp_required_for_next_level(level: u32) -> u32 {
        let lvl = if level == 0 { 1 } else { level };
        lvl.saturating_mul(BASE_XP_PER_LEVEL)
    }

    /// XP required for this character's next level-up.
    #[must_use]
    pub const fn xp_to_next_level(&self) -> u32 {
        Self::xp_required_for_next_level(self.level)
    }

    /// Grants `amount` XP to the character, automatically processing one or more level-ups
    /// and awarding [`ATTRIBUTE_POINTS_PER_LEVEL`] and [`SKILL_POINTS_PER_LEVEL`] per level gained.
    ///
    /// Returns the number of levels gained (`0` if no level-up occurred).
    pub fn gain_xp(&mut self, amount: u32) -> u32 {
        if amount == 0 {
            return 0;
        }
        self.current_xp = self.current_xp.saturating_add(amount);
        self.total_xp_earned = self.total_xp_earned.saturating_add(u64::from(amount));

        let mut levels_gained = 0;
        loop {
            let required = Self::xp_required_for_next_level(self.level);
            if self.current_xp >= required {
                self.current_xp -= required;
                self.level = self.level.saturating_add(1);
                self.unspent_attribute_points = self
                    .unspent_attribute_points
                    .saturating_add(ATTRIBUTE_POINTS_PER_LEVEL);
                self.unspent_skill_points = self
                    .unspent_skill_points
                    .saturating_add(SKILL_POINTS_PER_LEVEL);
                levels_gained += 1;
            } else {
                break;
            }
        }
        levels_gained
    }

    /// Spends `points` unspent attribute points on the specified [`AttributeStat`].
    pub fn allocate_attribute(
        &mut self,
        stat: AttributeStat,
        points: u32,
    ) -> Result<(), ProgressionError> {
        if points == 0 {
            return Ok(());
        }
        if self.unspent_attribute_points < points {
            return Err(ProgressionError::InsufficientAttributePoints {
                requested: points,
                available: self.unspent_attribute_points,
            });
        }
        self.unspent_attribute_points -= points;
        self.attributes.add(stat, points);
        Ok(())
    }

    /// Checks whether `skill_id` can currently be unlocked in `skill_tree`.
    pub fn can_unlock_skill<'a>(
        &self,
        skill_id: &str,
        skill_tree: &'a SkillTree,
    ) -> Result<&'a SkillNode, ProgressionError> {
        let Some(node) = skill_tree.get(skill_id) else {
            return Err(ProgressionError::UnknownSkill(skill_id.to_string()));
        };
        if self.unlocked_skills.contains(skill_id) {
            return Err(ProgressionError::SkillAlreadyUnlocked(skill_id.to_string()));
        }
        if self.level < node.required_level {
            return Err(ProgressionError::InsufficientLevelForSkill {
                skill_id: skill_id.to_string(),
                required_level: node.required_level,
                current_level: self.level,
            });
        }
        if self.unspent_skill_points < node.cost_points {
            return Err(ProgressionError::InsufficientSkillPoints {
                skill_id: skill_id.to_string(),
                cost: node.cost_points,
                available: self.unspent_skill_points,
            });
        }

        if !node.prerequisites.is_empty() {
            if node.requires_any_prerequisite {
                let any_met = node
                    .prerequisites
                    .iter()
                    .any(|p| self.unlocked_skills.contains(p));
                if !any_met {
                    return Err(ProgressionError::MissingSkillPrerequisites {
                        skill_id: skill_id.to_string(),
                        missing: node.prerequisites.clone(),
                    });
                }
            } else {
                let missing: Vec<String> = node
                    .prerequisites
                    .iter()
                    .filter(|p| !self.unlocked_skills.contains(*p))
                    .cloned()
                    .collect();
                if !missing.is_empty() {
                    return Err(ProgressionError::MissingSkillPrerequisites {
                        skill_id: skill_id.to_string(),
                        missing,
                    });
                }
            }
        }

        Ok(node)
    }

    /// Unlocks `skill_id` in the branching [`SkillTree`], deducting its skill point cost.
    pub fn unlock_skill(
        &mut self,
        skill_id: &str,
        skill_tree: &SkillTree,
    ) -> Result<(), ProgressionError> {
        let node = self.can_unlock_skill(skill_id, skill_tree)?;
        self.unspent_skill_points -= node.cost_points;
        self.unlocked_skills.insert(node.id.clone());
        Ok(())
    }

    /// Computes total [`PassiveSkillBonuses`] from all currently unlocked nodes in `skill_tree`.
    #[must_use]
    pub fn unlocked_passive_bonuses(&self, skill_tree: &SkillTree) -> PassiveSkillBonuses {
        let mut bonuses = PassiveSkillBonuses::default();
        for skill_id in &self.unlocked_skills {
            if let Some(node) = skill_tree.get(skill_id) {
                if let SkillEffect::PassiveStats {
                    attack_bonus,
                    speed_bonus,
                    defense_bonus,
                    max_health_bonus,
                    crit_chance_bonus,
                } = node.effect
                {
                    bonuses.attack_bonus += attack_bonus;
                    bonuses.speed_bonus += speed_bonus;
                    bonuses.defense_bonus += defense_bonus;
                    bonuses.max_health_bonus += max_health_bonus;
                    bonuses.crit_chance_bonus += crit_chance_bonus;
                }
            }
        }
        bonuses
    }

    /// Returns all [`ActiveSkillKind`]s unlocked by the character in `skill_tree`.
    #[must_use]
    pub fn unlocked_active_skills(&self, skill_tree: &SkillTree) -> Vec<ActiveSkillKind> {
        let mut actives = Vec::new();
        for skill_id in &self.unlocked_skills {
            if let Some(node) = skill_tree.get(skill_id) {
                if let SkillEffect::ActiveSkill(kind) = node.effect {
                    actives.push(kind);
                }
            }
        }
        actives
    }

    /// Returns the strongest unlocked [`ActiveSkillKind::Dash`] parameters, if unlocked.
    #[must_use]
    pub fn unlocked_dash_skill(&self, skill_tree: &SkillTree) -> Option<(f32, f32, f32)> {
        for kind in self.unlocked_active_skills(skill_tree) {
            if let ActiveSkillKind::Dash {
                distance_px,
                cooldown_secs,
                invulnerability_secs,
            } = kind
            {
                return Some((distance_px, cooldown_secs, invulnerability_secs));
            }
        }
        None
    }

    /// Returns the strongest unlocked [`ActiveSkillKind::AreaAttack`] parameters, if unlocked.
    #[must_use]
    pub fn unlocked_aoe_skill(&self, skill_tree: &SkillTree) -> Option<(f32, f32, i32, f32)> {
        let mut best: Option<(f32, f32, i32, f32)> = None;
        for kind in self.unlocked_active_skills(skill_tree) {
            if let ActiveSkillKind::AreaAttack {
                radius_px,
                damage_multiplier,
                flat_damage_bonus,
                cooldown_secs,
            } = kind
            {
                let replace = best.is_none_or(|(_, prev_mult, prev_flat, _)| {
                    damage_multiplier * 100.0 + flat_damage_bonus as f32
                        > prev_mult * 100.0 + prev_flat as f32
                });
                if replace {
                    best = Some((
                        radius_px,
                        damage_multiplier,
                        flat_damage_bonus,
                        cooldown_secs,
                    ));
                }
            }
        }
        best
    }

    /// Equips a weapon by ID after validating its existence and `required_level`.
    pub fn equip_weapon(
        &mut self,
        weapon_id: &str,
        registries: &GameContentRegistries,
    ) -> Result<(), ProgressionError> {
        let Some(resolved_id) = resolve_weapon_id(weapon_id, registries) else {
            return Err(ProgressionError::UnknownWeapon(weapon_id.to_string()));
        };
        let weapon_def = registries
            .weapons
            .get(&resolved_id)
            .expect("resolved weapon ID exists");
        if self.level < weapon_def.required_level {
            return Err(ProgressionError::InsufficientLevelForWeapon {
                weapon_id: resolved_id,
                required_level: weapon_def.required_level,
                current_level: self.level,
            });
        }
        self.equipped_weapon_id = resolved_id.clone();
        if !self.inventory.contains(&resolved_id) {
            self.inventory.push(resolved_id);
        }
        Ok(())
    }

    /// Evolves the currently equipped weapon to the next stage in its evolution path.
    ///
    /// - If `target_weapon_id` is `None`, evolves to the first successor in `evolves_to`.
    /// - If `target_weapon_id` is `Some(id)`, validates that `id` is a valid branch in `evolves_to`.
    /// - Enforces that `self.level >= target_weapon.required_level`.
    pub fn evolve_weapon(
        &mut self,
        target_weapon_id: Option<&str>,
        registries: &GameContentRegistries,
    ) -> Result<String, ProgressionError> {
        let Some(current_def) = registries.weapons.get(&self.equipped_weapon_id) else {
            return Err(ProgressionError::UnknownWeapon(
                self.equipped_weapon_id.clone(),
            ));
        };

        if current_def.evolves_to.is_empty() {
            return Err(ProgressionError::NoFurtherWeaponEvolution(
                self.equipped_weapon_id.clone(),
            ));
        }

        let chosen_id = match target_weapon_id {
            Some(target) => {
                if !current_def.evolves_to.iter().any(|e| e == target) {
                    return Err(ProgressionError::InvalidEvolutionTarget {
                        current_weapon: self.equipped_weapon_id.clone(),
                        target_weapon: target.to_string(),
                    });
                }
                target.to_string()
            }
            None => current_def.evolves_to[0].clone(),
        };

        let Some(target_def) = registries.weapons.get(&chosen_id) else {
            return Err(ProgressionError::UnknownWeapon(chosen_id));
        };

        if self.level < target_def.required_level {
            return Err(ProgressionError::InsufficientLevelForWeapon {
                weapon_id: target_def.id.clone(),
                required_level: target_def.required_level,
                current_level: self.level,
            });
        }

        self.equipped_weapon_id = target_def.id.clone();
        if !self.inventory.contains(&target_def.id) {
            self.inventory.push(target_def.id.clone());
        }
        Ok(target_def.id.clone())
    }

    /// Retrieves the currently equipped [`WeaponDefinition`] from `registries`.
    #[must_use]
    pub fn current_weapon<'a>(
        &self,
        registries: &'a GameContentRegistries,
    ) -> Option<&'a WeaponDefinition> {
        resolve_weapon_id(&self.equipped_weapon_id, registries)
            .and_then(|id| registries.weapons.get(&id))
    }

    /// Computes the character's [`EffectiveStats`] from class, level, attributes, skilltree, and weapon.
    #[must_use]
    pub fn compute_effective_stats(
        &self,
        registries: &GameContentRegistries,
        skill_tree: &SkillTree,
    ) -> EffectiveStats {
        let (class_hp, class_mana, class_speed, trait_atk, trait_def, trait_spd) = registries
            .classes
            .get(&self.class_id)
            .map_or((100, 40, 64.0, 0, 0, 0.0), |c| {
                let (t_atk, t_def, t_spd) = match c.passive_trait.as_str() {
                    "VanguardEdge" => (5, 2, 0.0),
                    "NanoAegis" => (2, 6, 0.0),
                    "Snap Step" => (3, 1, 4.0),
                    "ChromaticBrew" => (4, 2, 2.0),
                    _ => (2, 2, 0.0),
                };
                (
                    c.base_health,
                    c.base_mana,
                    c.base_speed,
                    t_atk,
                    t_def,
                    t_spd,
                )
            });

        let levels_above_one = self.level.saturating_sub(1) as i32;
        let skill_bonuses = self.unlocked_passive_bonuses(skill_tree);

        let (weapon_dmg, weapon_spd, weapon_range, weapon_crit) =
            self.current_weapon(registries).map_or_else(
                || {
                    let item_bonus = registries
                        .items
                        .get(&self.equipped_weapon_id)
                        .map_or(6, |i| i.power_bonus.max(1));
                    (item_bonus, 1.4, 18.0, 0.05)
                },
                |w| (w.base_damage, w.attack_speed, w.range_px, w.crit_chance),
            );

        let max_health = (class_hp
            + levels_above_one * 20
            + (self.attributes.defense as i32) * 8
            + (self.attributes.vitality as i32) * 15
            + skill_bonuses.max_health_bonus)
            .max(1);

        let max_mana = (class_mana + levels_above_one * 6).max(0);

        let attack = (10
            + trait_atk
            + levels_above_one * 5
            + (self.attributes.attack as i32) * 3
            + skill_bonuses.attack_bonus
            + weapon_dmg)
            .max(1);

        let defense = (4
            + trait_def
            + levels_above_one * 3
            + (self.attributes.defense as i32) * 2
            + (self.attributes.vitality as i32)
            + skill_bonuses.defense_bonus)
            .max(0);

        let speed = (class_speed
            + trait_spd
            + (levels_above_one as f32) * 1.2
            + (self.attributes.speed as f32) * 2.5
            + skill_bonuses.speed_bonus)
            .max(16.0);

        let crit_chance =
            (weapon_crit + (self.attributes.speed as f32) * 0.01 + skill_bonuses.crit_chance_bonus)
                .clamp(0.0, 0.95);

        EffectiveStats {
            max_health,
            max_mana,
            attack,
            defense,
            speed,
            crit_chance,
            attack_range_px: weapon_range,
            attack_speed: weapon_spd,
        }
    }
}

/// Resolves a weapon or weapon-item identifier (e.g. `"weapon.rusty_dagger"` or `"item.pastel_sword"`)
/// to its canonical key in `registries.weapons`.
#[must_use]
pub fn resolve_weapon_id(id: &str, registries: &GameContentRegistries) -> Option<String> {
    if registries.weapons.get(id).is_some() {
        return Some(id.to_string());
    }
    if let Some(stripped) = id.strip_prefix("item.") {
        let candidate = format!("weapon.{stripped}");
        if registries.weapons.get(&candidate).is_some() {
            return Some(candidate);
        }
    }
    None
}

/// Event emitted when the player levels up.
#[derive(Event, Debug, Clone, PartialEq, Eq)]
pub struct LevelUpEvent {
    pub old_level: u32,
    pub new_level: u32,
    pub attribute_points_gained: u32,
    pub skill_points_gained: u32,
}

/// Event emitted when a player evolves their weapon.
#[derive(Event, Debug, Clone, PartialEq, Eq)]
pub struct WeaponEvolvedEvent {
    pub previous_weapon_id: String,
    pub new_weapon_id: String,
}
