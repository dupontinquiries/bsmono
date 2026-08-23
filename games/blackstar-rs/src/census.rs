//! THE CENSUS AS A CENSUS — most bots should not have lore.
//!
//! `tools/meta/bots.json` places 259 screened models: a class, a home world,
//! a faction, and a one-line hook. Its own documentation is unusually honest
//! about the state of that last field:
//!
//! > 89 distinct hooks across 259 bots. Fifty-six grimorphs share one line,
//! > forty flyers share another... the other 178 read as their class, which
//! > is placement metadata wearing a costume, not character.
//!
//! The obvious response is to write 178 more hooks. That is the wrong
//! response, and not only because it is a lot of adjectives: **a population
//! is not two hundred and fifty-nine characters.** A world with 259 named
//! personalities has no crowd in it, and the crowd is what makes the named
//! ones legible.
//!
//! So this module treats the roster as a CENSUS. Individual bots mostly do
//! not mean anything. The DISTRIBUTION does, and it turns out to mean rather
//! a lot, because the placement rules were written against the film
//! material's geography and reading them back gives statements nobody typed
//! out.
//!
//! ## A CORRECTION, kept where it happened
//!
//! An earlier version of this module observed that Cybertron has fifty-six
//! frames and not one that stands out, and read it as characterisation: a
//! place where the difference between one frame and another stopped
//! mattering. **That was meaning drawn from a data artifact.** The ids are
//! `grimorph_part4` through `grimorph_part88` — pieces of one exploded model,
//! listed as if each were a bot. Cybertron does not have fifty-six anonymous
//! residents; it has none, and a parts bin standing where they should be.
//! `cybertronian.rs` is the frames that belong there, and its
//! `the_parts_bin_is_not_a_population` pins the correction.
//!
//! The rest of the census below survives that, because the other facts are
//! about placement rules rather than about model files.
//!
//! ## What the census says
//!
//! **All fifty-six unrecorded shapes are on Cybertron, and every one is
//! Corrupted.** The hook they share — *"a shape the coalition has no record
//! of"* — is already a provenance statement rather than a character note. And
//! `archive.rs` has the matching claim from the other direction:
//! `substrate_sample`, sited on Cybertron, says the energy architecture there
//! is older than the planet's own record of inventing it. Fifty-six frames
//! nobody can account for, all in one place, all wrong, is that claim seen
//! from the population side.
//!
//! **Every one of the forty-six on Earth is Coalition, and every one is a
//! road vehicle** — cars, trucks, racers, a few helicopters. No grimorphs, no
//! spaceships, nothing that could not be parked. That is concealment as a
//! demographic fact, and it is what a human investigator would notice first.
//!
//! ## How it reaches the map
//!
//! As `Bearing::Supports` from ONE source. A census is a real observation and
//! not hearsay — unlike `palevector`, which only ever `Mentions` — but it is
//! still one body of evidence, so on its own it lands a claim at UNKNOWN and
//! waits for something independent. That is exactly right: counting bots is
//! suggestive and it is not a dig.

use crate::provenance::{Bearing, Prior};

/// One placed model. Deliberately thin: id, what it is, where it lives, whose
/// it is, and whether its hook is its own or shared with fifty-five others.
pub struct Bot {
    pub id: &'static str,
    pub class: &'static str,
    pub home: &'static str,
    pub faction: &'static str,
    /// its hook belongs to it alone
    pub distinct: bool,
}

const fn b(
    id: &'static str,
    class: &'static str,
    home: &'static str,
    faction: &'static str,
    distinct: bool,
) -> Bot {
    Bot { id, class, home, faction, distinct }
}
use b as B;

/// Baked from `tools/meta/bots.json`; `the_census_matches_its_source` keeps
/// the two honest.
pub static CENSUS: &[Bot] = &[
    B("animal_bot_dimstreak","animal","glaciem","Coalition",false),
    B("car_1968_alfa_romeo_33_stradale","car","earth","Coalition",false),
    B("car_1970_alfa_romeo_montreal","car","earth","Coalition",false),
    B("car_1970_dodge_charger_rt_fast_furious","car","earth","Coalition",false),
    B("car_1970_plymouth_barracuda_440_6","car","earth","Coalition",false),
    B("car_1977_renault_alpine_a442","car","earth","Coalition",false),
    B("car_2019_porsche_718_boxster_t","car","earth","Coalition",false),
    B("car_2024_lbsilhouette_works_murcielago_gt_evo","car","earth","Coalition",false),
    B("car_2026_ferrari_849_testarossa","car","earth","Coalition",false),
    B("car_alfa_romeo_disco_volante_speziale_coupe","car","earth","Coalition",false),
    B("car_bac_mono","car","earth","Coalition",false),
    B("car_box","car","earth","Coalition",false),
    B("car_bruiser_aka_special_ops_racer","car","earth","Coalition",true),
    B("car_death_patrol_aka_nissan_350z_rocket_bunny","car","earth","Coalition",true),
    B("car_deathreaver_aka_the_maze_inspired_offroader","car","earth","Coalition",true),
    B("car_deepspeed_aka_ben_10_inspired_muscle_car","car","earth","Coalition",true),
    B("car_ford_falcon_gt_1973","car","helix","Coalition",false),
    B("car_hot_wheels_twin_mill","car","earth","Coalition",false),
    B("car_hunter_manhattan_custom","car","earth","Coalition",false),
    B("car_intake_aka_turbine_car_race_car_b","car","earth","Coalition",true),
    B("car_lowrider_aka_buick_riviera_1963_1965_www_vecarz_com","car","earth","Coalition",true),
    B("car_lynx_crabronix_dash_v","car","earth","Coalition",false),
    B("car_mad_max_desert_rust","car","earth","Coalition",false),
    B("car_madmax_sports_car","car","earth","Coalition",false),
    B("car_nanohound_aka_pentagon_nanowheel_futuristic_hover_car","car","earth","Coalition",true),
    B("car_orion_skylark_gt","car","earth","Coalition",false),
    B("car_outback_aka_jeep_scifi_lav_d3","car","earth","Coalition",true),
    B("car_prowler","car","earth","Coalition",false),
    B("car_racing_2004_ferrari_f2004","car","earth","Coalition",false),
    B("car_rolodex_aka_futuristic_sleek_mercedes_benz_silver_lightning","car","earth","Coalition",true),
    B("car_rumbleshock_aka_futuristic_car_rigged","car","earth","Coalition",true),
    B("car_speed_racers_mach_6","car","earth","Coalition",false),
    B("car_sprocket_aka_edwardian_era_tour_tractor_hybrid_sprocket_rocket","car","earth","Coalition",true),
    B("car_widowmaker_death_race_2000","car","carrion","Corrupted",false),
    B("dragonflyer_ironforge_aka_cyber_dragon_warrior_bot_with_hand_and_chest_beam_blaster","dragonflyer","helix","Coalition",true),
    B("flyer_2_modes_viper","flyer","shield_edge","Hijacks",false),
    B("flyer_alphastar","flyer","shield_edge","Hijacks",false),
    B("flyer_argentglide_aka_s_er","flyer","shield_edge","Hijacks",false),
    B("flyer_beetlewing_aka_the_x_bow_hover_craft","flyer","shield_edge","Hijacks",true),
    B("flyer_bulwark","flyer","shield_edge","Hijacks",false),
    B("flyer_cool_futuristic_flyer","flyer","shield_edge","Hijacks",false),
    B("flyer_darkguard","flyer","shield_edge","Hijacks",false),
    B("flyer_dawnshade","flyer","shield_edge","Hijacks",false),
    B("flyer_deathstroke","flyer","shield_edge","Hijacks",false),
    B("flyer_doomlift","flyer","shield_edge","Hijacks",false),
    B("flyer_forlorn_aka_av_8b_na_prototype","flyer","shield_edge","Hijacks",true),
    B("flyer_freighter","flyer","shield_edge","Hijacks",false),
    B("flyer_hull","flyer","shield_edge","Hijacks",false),
    B("flyer_influx_aka_mg_harrier_defender_121_116_00","flyer","shield_edge","Hijacks",true),
    B("flyer_interceptor","flyer","shield_edge","Hijacks",false),
    B("flyer_intervention","flyer","shield_edge","Hijacks",false),
    B("flyer_kfir_c10","flyer","shield_edge","Hijacks",false),
    B("flyer_lim_6bis_poland","flyer","shield_edge","Hijacks",false),
    B("flyer_lonestar_aka_bronze_space_age_combat_jet_animation","flyer","shield_edge","Hijacks",true),
    B("flyer_mikoyan_mig_25pd_ussr","flyer","shield_edge","Hijacks",false),
    B("flyer_missile_human","flyer","shield_edge","Hijacks",false),
    B("flyer_needlenose_aka_chinese_chengdu_j_20","flyer","shield_edge","Hijacks",true),
    B("flyer_nimbus","flyer","shield_edge","Hijacks",false),
    B("flyer_pelican","flyer","shield_edge","Hijacks",false),
    B("flyer_perception","flyer","shield_edge","Hijacks",false),
    B("flyer_prowler","flyer","shield_edge","Hijacks",false),
    B("flyer_ps1low_poly_f_16_fighter_jet_w_animations","flyer","shield_edge","Hijacks",false),
    B("flyer_pursuer","flyer","shield_edge","Hijacks",false),
    B("flyer_red_dot","flyer","shield_edge","Hijacks",false),
    B("flyer_resonator","flyer","shield_edge","Hijacks",false),
    B("flyer_sea_harrier_fa2_with_aim_9l_aim_120b","flyer","shield_edge","Hijacks",false),
    B("flyer_shadowscream","flyer","shield_edge","Hijacks",false),
    B("flyer_skynok","flyer","shield_edge","Hijacks",false),
    B("flyer_skytower","flyer","shield_edge","Hijacks",false),
    B("flyer_spaceship_ufo_2","flyer","shield_edge","Hijacks",false),
    B("flyer_sparrowhawk","flyer","helix","Coalition",false),
    B("flyer_speedrunner","flyer","shield_edge","Hijacks",false),
    B("flyer_splinter","flyer","shield_edge","Hijacks",false),
    B("flyer_stardetris_aka_velocity_90","flyer","shield_edge","Hijacks",true),
    B("flyer_tercel","flyer","shield_edge","Hijacks",false),
    B("flyer_updraft","flyer","shield_edge","Hijacks",false),
    B("flyer_valkyrie","flyer","shield_edge","Hijacks",false),
    B("flyer_ventride","flyer","shield_edge","Hijacks",false),
    B("flyer_vertigo","flyer","shield_edge","Hijacks",false),
    B("flyer_wayside","flyer","shield_edge","Hijacks",false),
    B("flyer_whisperwind_aka_f_117_nighthawk_usa","flyer","helix","Coalition",true),
    B("flyer_white_tiger","flyer","shield_edge","Hijacks",false),
    B("grimorph_part10","grimorph","cybertron","Corrupted",false),
    B("grimorph_part11","grimorph","cybertron","Corrupted",false),
    B("grimorph_part12","grimorph","cybertron","Corrupted",false),
    B("grimorph_part13","grimorph","cybertron","Corrupted",false),
    B("grimorph_part16","grimorph","cybertron","Corrupted",false),
    B("grimorph_part17","grimorph","cybertron","Corrupted",false),
    B("grimorph_part18","grimorph","cybertron","Corrupted",false),
    B("grimorph_part20","grimorph","cybertron","Corrupted",false),
    B("grimorph_part21","grimorph","cybertron","Corrupted",false),
    B("grimorph_part22","grimorph","cybertron","Corrupted",false),
    B("grimorph_part23","grimorph","cybertron","Corrupted",false),
    B("grimorph_part24","grimorph","cybertron","Corrupted",false),
    B("grimorph_part25","grimorph","cybertron","Corrupted",false),
    B("grimorph_part26","grimorph","cybertron","Corrupted",false),
    B("grimorph_part27","grimorph","cybertron","Corrupted",false),
    B("grimorph_part30","grimorph","cybertron","Corrupted",false),
    B("grimorph_part31","grimorph","cybertron","Corrupted",false),
    B("grimorph_part32","grimorph","cybertron","Corrupted",false),
    B("grimorph_part36","grimorph","cybertron","Corrupted",false),
    B("grimorph_part38","grimorph","cybertron","Corrupted",false),
    B("grimorph_part4","grimorph","cybertron","Corrupted",false),
    B("grimorph_part40","grimorph","cybertron","Corrupted",false),
    B("grimorph_part42","grimorph","cybertron","Corrupted",false),
    B("grimorph_part43","grimorph","cybertron","Corrupted",false),
    B("grimorph_part44","grimorph","cybertron","Corrupted",false),
    B("grimorph_part45","grimorph","cybertron","Corrupted",false),
    B("grimorph_part48","grimorph","cybertron","Corrupted",false),
    B("grimorph_part49","grimorph","cybertron","Corrupted",false),
    B("grimorph_part5","grimorph","cybertron","Corrupted",false),
    B("grimorph_part50","grimorph","cybertron","Corrupted",false),
    B("grimorph_part52","grimorph","cybertron","Corrupted",false),
    B("grimorph_part53","grimorph","cybertron","Corrupted",false),
    B("grimorph_part54","grimorph","cybertron","Corrupted",false),
    B("grimorph_part55","grimorph","cybertron","Corrupted",false),
    B("grimorph_part56","grimorph","cybertron","Corrupted",false),
    B("grimorph_part58","grimorph","cybertron","Corrupted",false),
    B("grimorph_part59","grimorph","cybertron","Corrupted",false),
    B("grimorph_part63","grimorph","cybertron","Corrupted",false),
    B("grimorph_part64","grimorph","cybertron","Corrupted",false),
    B("grimorph_part66","grimorph","cybertron","Corrupted",false),
    B("grimorph_part69","grimorph","cybertron","Corrupted",false),
    B("grimorph_part7","grimorph","cybertron","Corrupted",false),
    B("grimorph_part71","grimorph","cybertron","Corrupted",false),
    B("grimorph_part72","grimorph","cybertron","Corrupted",false),
    B("grimorph_part73","grimorph","cybertron","Corrupted",false),
    B("grimorph_part75","grimorph","cybertron","Corrupted",false),
    B("grimorph_part76","grimorph","cybertron","Corrupted",false),
    B("grimorph_part77","grimorph","cybertron","Corrupted",false),
    B("grimorph_part78","grimorph","cybertron","Corrupted",false),
    B("grimorph_part8","grimorph","cybertron","Corrupted",false),
    B("grimorph_part80","grimorph","cybertron","Corrupted",false),
    B("grimorph_part82","grimorph","cybertron","Corrupted",false),
    B("grimorph_part84","grimorph","cybertron","Corrupted",false),
    B("grimorph_part85","grimorph","cybertron","Corrupted",false),
    B("grimorph_part88","grimorph","cybertron","Corrupted",false),
    B("grimorph_part9","grimorph","cybertron","Corrupted",false),
    B("hawk_flyer_asonia","hawk","helix","Coalition",false),
    B("hawk_flyer_falcontip_aka_hawk_mech_osprey_x_80","hawk","helix","Coalition",true),
    B("heli_eclipse_aka_z_10ag_attack_helicopter","heli","earth","Coalition",true),
    B("heli_hornet_aka_z_29_sea_hornet_drone_copter","heli","earth","Coalition",true),
    B("heli_stiletto_aka_future_drone_1","heli","earth","Coalition",true),
    B("hoverer_a8_glider_hovercar","hoverer","titan","BlackstarClans",false),
    B("hoverer_analog_aka_weaponless_scout","hoverer","titan","BlackstarClans",true),
    B("hoverer_beetlejuice","hoverer","titan","BlackstarClans",false),
    B("hoverer_blackstar","hoverer","titan","BlackstarClans",false),
    B("hoverer_brutis_aka_hover_tank","hoverer","titan","BlackstarClans",true),
    B("hoverer_bullhorn","hoverer","titan","BlackstarClans",false),
    B("hoverer_chromoneon_aka_blnk_100_concept_hovering_car","hoverer","titan","BlackstarClans",true),
    B("hoverer_contingency","hoverer","titan","BlackstarClans",false),
    B("hoverer_elegance","hoverer","titan","BlackstarClans",false),
    B("hoverer_folly_aka_land_surfer_sharp","hoverer","titan","BlackstarClans",true),
    B("hoverer_hardwall_aka_dystopian_police_car","hoverer","titan","BlackstarClans",true),
    B("hoverer_lineback_aka_modular_hover_racers_example","hoverer","titan","BlackstarClans",true),
    B("hoverer_lowride","hoverer","titan","BlackstarClans",false),
    B("hoverer_matris_aka_elegant_hovercar","hoverer","titan","BlackstarClans",true),
    B("hoverer_octotiger_aka_drag_racing_dodge_charger_hover_concept","hoverer","titan","BlackstarClans",true),
    B("hoverer_pinprick_aka_missile_barrage_hovercar","hoverer","titan","BlackstarClans",true),
    B("hoverer_prowlshock_aka_hover_bike_car","hoverer","titan","BlackstarClans",true),
    B("hoverer_revenant_forlorn_aka_retro_hovercraft","hoverer","titan","BlackstarClans",true),
    B("hoverer_rosenbridge_aka_futuristic_racing_car_molnia","hoverer","titan","BlackstarClans",true),
    B("hoverer_rubble_aka_hover_tank_scifi","hoverer","titan","BlackstarClans",true),
    B("hoverer_solstice_aka_futuristic_sleek_hovercraft_car","hoverer","titan","BlackstarClans",true),
    B("hoverer_steelius","hoverer","titan","BlackstarClans",false),
    B("hoverer_trifang","hoverer","titan","BlackstarClans",false),
    B("hoverer_turismo_aka_hover_car_cyberpunk","hoverer","titan","BlackstarClans",true),
    B("hoverer_unitus_aka_free_sci_fi_vehicle_001_public_domain_cc0","hoverer","titan","BlackstarClans",true),
    B("hoverer_vectorzoid_aka_futuristic_shielded_hovercar","hoverer","titan","BlackstarClans",true),
    B("hoverer_vulcanus","hoverer","titan","BlackstarClans",false),
    B("mech_constructor_heavy_spider_animated","mech","carrion","Corrupted",false),
    B("mech_constructor_the_defender","mech","uncharted","Hijacks",false),
    B("mech_void","mech","uncharted","Hijacks",false),
    B("moto_arachnovent_aka_futuristic_alien_motorcycle","moto","carrion","Corrupted",true),
    B("moto_darkwolf_aka_shelled_cyberpunk_motorcycle","moto","mars","Hijacks",true),
    B("moto_greynoise_aka_desaturated_thorium_motorcycle","moto","mars","Hijacks",true),
    B("moto_kaput_aka_russian_motorcycle_3dscan","moto","mars","Hijacks",true),
    B("moto_ribcage_aka_postapo_motorcycle_low","moto","mars","Hijacks",true),
    B("moto_skullyotyl_aka_exhaust_and_bones_motorcycle","moto","mars","Hijacks",true),
    B("moto_temporus_aka_time_traveler_steampunk_motorcycle","moto","mars","Hijacks",true),
    B("racer_cryofin_aka_slick_mobile","racer","earth","Coalition",true),
    B("racer_grit_aka_rocket_car","racer","earth","Coalition",true),
    B("racer_nightvent","racer","earth","Coalition",false),
    B("racer_quickfire","racer","earth","Coalition",false),
    B("raidboss_hardboil","raidboss","outer_rim","BlackstarClans",false),
    B("raidboss_hussar_tank","raidboss","outer_rim","BlackstarClans",false),
    B("raidboss_judgementor_aka_large_walker_tank","raidboss","outer_rim","BlackstarClans",true),
    B("raidboss_primitive_mantis_mech","raidboss","outer_rim","BlackstarClans",false),
    B("raidboss_pyrachnid","raidboss","outer_rim","BlackstarClans",false),
    B("raidboss_scorpion_commander_transformer_reference_remove_the_hat_find_a_scorpion_body","raidboss","outer_rim","BlackstarClans",false),
    B("raidboss_starlord_aka_human_sized_transformer_with_two_hammers_that_are_sentry_guns","raidboss","outer_rim","BlackstarClans",true),
    B("raidboss_strider_tank","raidboss","outer_rim","BlackstarClans",false),
    B("raidboss_trenchmaster","raidboss","outer_rim","BlackstarClans",false),
    B("spaceship_albatar","spaceship","outer_rim","Coalition",false),
    B("spaceship_ascendus","spaceship","outer_rim","Coalition",false),
    B("spaceship_briskfall","spaceship","outer_rim","Coalition",false),
    B("spaceship_cargo_01","spaceship","outer_rim","Coalition",false),
    B("spaceship_horizon","spaceship","outer_rim","Coalition",false),
    B("spaceship_marmot_aka_att_multi_troop_transport","spaceship","outer_rim","Coalition",true),
    B("spaceship_nemesis","spaceship","outer_rim","Coalition",false),
    B("spaceship_odyssey","spaceship","outer_rim","Coalition",false),
    B("spaceship_patronus_black","spaceship","outer_rim","Coalition",false),
    B("spaceship_pax_wind","spaceship","outer_rim","Coalition",false),
    B("spaceship_pinnacle","spaceship","outer_rim","Coalition",false),
    B("spaceship_praetor_aka_spaceship_with_partial_interior","spaceship","outer_rim","Coalition",true),
    B("spaceship_rocket_ship","spaceship","outer_rim","Coalition",false),
    B("spaceship_small_hawkdrop_aka_spaceship_retopology_and_texturing","spaceship","outer_rim","Coalition",true),
    B("spaceship_spearhead_aka_rocket_spaceship_concept_animated","spaceship","outer_rim","Coalition",true),
    B("spaceship_supervac","spaceship","outer_rim","Coalition",false),
    B("spaceship_transformers_fall_of_cybertron_ark","spaceship","outer_rim","Coalition",false),
    B("spaceship_unionizer","spaceship","outer_rim","Coalition",false),
    B("spaceship_unitron","spaceship","outer_rim","Coalition",false),
    B("spaceship_unsc_vorpal_blade_palomino_class_halo_zeo","spaceship","outer_rim","Coalition",false),
    B("spaceship_warpzone_aka_military_troop_ship","spaceship","outer_rim","Coalition",true),
    B("spider_drone_v6_cybertech","spider","uncharted","Hijacks",false),
    B("spider_tank","spider","uncharted","Hijacks",false),
    B("spiderbot_radiachnor","spiderbot","uncharted","Hijacks",false),
    B("spiderbot_widowback_aka_sci_fi_spider_metal_pbr_low_poly_game_ready_3d_m","spiderbot","carrion","Corrupted",true),
    B("tank_alpine_doom","tank","io","BlackstarClans",false),
    B("tank_ariete_c1","tank","io","BlackstarClans",false),
    B("tank_blackshell_aka_cyber_tank_futuristic_cyberpunk","tank","io","BlackstarClans",true),
    B("tank_boartail","tank","io","BlackstarClans",false),
    B("tank_chokepoint_aka_starcraft_inspired_siege_tank_tribute","tank","io","BlackstarClans",true),
    B("tank_colossus_aka_halo_scorpion_style_tank","tank","io","BlackstarClans",true),
    B("tank_crusher_aka_grob_traktor","tank","io","BlackstarClans",true),
    B("tank_dante_aka_quad_erergon_plasma_flamethrower_tank","tank","io","BlackstarClans",true),
    B("tank_ebr_105_mad_max_mod","tank","io","BlackstarClans",false),
    B("tank_ekmos_aka_ek23_tank_gun3","tank","io","BlackstarClans",true),
    B("tank_embermane_aka_tank_02_sci_fi_lp","tank","io","BlackstarClans",true),
    B("tank_fiat2000_italian_heavy_tank_parade_version","tank","io","BlackstarClans",false),
    B("tank_fivesix","tank","io","BlackstarClans",false),
    B("tank_greenpath_aka_ats_103_north_korea","tank","io","BlackstarClans",true),
    B("tank_guzzle_aka_military_aav","tank","io","BlackstarClans",true),
    B("tank_hawkeye_aka_modern_t_i_k_battle_tank","tank","helix","Coalition",true),
    B("tank_houndoom_aka_treaded_futuristic_laser_tank","tank","io","BlackstarClans",true),
    B("tank_incision_aka_decepticon_tank_m1978_koksan_north_korea","tank","io","BlackstarClans",true),
    B("tank_lockon","tank","io","BlackstarClans",false),
    B("tank_lynx_6x6","tank","io","BlackstarClans",false),
    B("tank_nightprowl_aka_the_m98a3_behomoth_concept","tank","io","BlackstarClans",true),
    B("tank_nsomad_aka_light_infantry_operations_nomad_a3_b_tank","tank","io","BlackstarClans",true),
    B("tank_patton_aka_scifitank_patton","tank","io","BlackstarClans",true),
    B("tank_pepperspray_aka_panzerkampfwagen_e_175_ausf_b_fledermaus","tank","io","BlackstarClans",true),
    B("tank_riot_aka_anti_riot_heavy_unit_futuristic_cyberpunk","tank","io","BlackstarClans",true),
    B("tank_rollout_aka_sci_fi_tank","tank","io","BlackstarClans",false),
    B("tank_t_35","tank","io","BlackstarClans",false),
    B("tank_t_70","tank","io","BlackstarClans",false),
    B("tank_tetradot_aka_nextg_tank","tank","io","BlackstarClans",true),
    B("tank_the_russian_bmd_4_based_rakushka","tank","io","BlackstarClans",false),
    B("tank_thermoblitzer_aka_sci_fi_tank","tank","io","BlackstarClans",false),
    B("tank_thunderboar_aka_metalclad_heavy_iron_stylized_tank","tank","io","BlackstarClans",true),
    B("tank_vanquish_aka_t_666_concept_tank","tank","io","BlackstarClans",true),
    B("truck_2003_peugeot_hoggar_concept","truck","earth","Coalition",false),
    B("truck_barrage_aka_m270_multiple_launch_rocket_system","truck","earth","Coalition",true),
    B("truck_btr_80","truck","earth","Coalition",false),
    B("truck_dino_truck_tonton","truck","earth","Coalition",false),
    B("truck_giant_elephant","truck","earth","Coalition",false),
    B("truck_mad_car_8k_resolution","truck","earth","Coalition",false),
    B("truck_maz_543","truck","earth","Coalition",false),
    B("truck_utilitarian_vehicle","truck","earth","Coalition",false),
];

/// The source this was baked from.
pub const SOURCE_JSON: &str = "tools/meta/bots.json";

/// One model by id, for when a bot does turn out to matter individually.
pub fn bot(id: &str) -> Option<&'static Bot> {
    CENSUS.iter().find(|b| b.id == id)
}

pub fn at(home: &str) -> Vec<&'static Bot> {
    CENSUS.iter().filter(|b| b.home == home).collect()
}

/// Counts for one world: (total, distinct hooks, factions present).
pub fn census(home: &str) -> (usize, usize, Vec<&'static str>) {
    let all = at(home);
    let distinct = all.iter().filter(|b| b.distinct).count();
    let mut f: Vec<&'static str> = all.iter().map(|b| b.faction).collect();
    f.sort_unstable();
    f.dedup();
    (all.len(), distinct, f)
}

/// Does every bot on this world belong to one faction?
pub fn monoculture(home: &str) -> Option<&'static str> {
    let (_, _, f) = census(home);
    (f.len() == 1).then(|| f[0])
}

/// What the population supports, for the archaeological map.
///
/// Derived from the distribution rather than authored. Each is one source, so
/// none of them can corroborate anything on its own.
pub fn census_findings() -> Vec<Prior> {
    let mut out = Vec::new();
    // every unrecorded shape in one place, and all of it corrupted
    let unrec: Vec<&Bot> = CENSUS.iter().filter(|b| b.class == "grimorph").collect();
    if !unrec.is_empty()
        && unrec.iter().all(|b| b.home == "cybertron")
        && unrec.iter().all(|b| b.faction == "Corrupted")
    {
        out.push(Prior {
            claim: "architecture_is_machine",
            bearing: Bearing::Supports,
            source: SOURCE_CENSUS,
        });
    }
    // everything on Earth is Coalition and shaped like an Earth vehicle
    let earth = at("earth");
    let roadable = ["car", "truck", "racer", "heli", "moto", "bus", "van"];
    if !earth.is_empty()
        && earth.iter().all(|b| b.faction == "Coalition")
        && earth.iter().all(|b| roadable.contains(&b.class))
    {
        out.push(Prior {
            claim: "societies_existed",
            bearing: Bearing::Supports,
            source: SOURCE_CENSUS,
        });
    }
    out
}

/// One body of evidence: a headcount.
pub const SOURCE_CENSUS: &str = "population_census";

// ------------------------------------------------------------- driver ---

pub fn run() {
    use crate::patrol::ROUTE;
    println!("THE ROSTER AS A CENSUS — {} placed models", CENSUS.len());
    println!("  baked from {SOURCE_JSON}\n");

    let mut homes: Vec<&str> = CENSUS.iter().map(|b| b.home).collect();
    homes.sort_unstable();
    homes.dedup();
    homes.sort_by_key(|h| std::cmp::Reverse(at(h).len()));

    println!("  {:<14} {:>6} {:>9} {:>10}  {}", "world", "bots", "own hook", "findings", "factions");
    for h in &homes {
        let (n, distinct, f) = census(h);
        let finds = crate::archive::FINDINGS.iter().filter(|x| x.site == *h).count();
        println!(
            "  {:<14} {:>6} {:>9} {:>10}  {}",
            h,
            n,
            distinct,
            if finds > 0 { finds.to_string() } else { "—".into() },
            f.join(", ")
        );
    }
    let on_route = homes.iter().filter(|h| ROUTE.iter().any(|d| d.id == **h)).count();
    println!("\n  {on_route} of {} homes are places you can fly to.", homes.len());

    println!("\n  MONOCULTURES — a world where everyone belongs to one side\n");
    for h in &homes {
        if let Some(f) = monoculture(h) {
            println!("    {:<14} all {} ({})", h, f, at(h).len());
        }
    }

    let mono = homes.iter().filter(|h| monoculture(h).is_some()).count();
    println!(
        "  {mono} of {} worlds are MONOCULTURES — one faction, nobody else. Only",
        homes.len()
    );
    println!("  outer_rim is contested, which is why the boundary is where the");
    println!("  story keeps happening.");

    println!("\n  WHAT THE DISTRIBUTION SAYS\n");
    let unrec = CENSUS.iter().filter(|b| b.class == "grimorph").count();
    println!("    {unrec} frames share the hook \"a shape the coalition has no record of\".");
    println!("    every one of them is on cybertron. every one is Corrupted.");
    let earth_classes: Vec<&str> = {
        let mut c: Vec<&str> = at("earth").iter().map(|b| b.class).collect();
        c.sort_unstable();
        c.dedup();
        c
    };
    println!("\n    and everything on earth is a {}.", earth_classes.join(", a "));
    println!("    nothing that could not be parked.");

    println!("\n  WHAT REACHES THE MAP\n");
    for p in census_findings() {
        let text = crate::archive::claim(p.claim).map(|c| c.text).unwrap_or("?");
        println!("    {:?}  {}", p.bearing, p.claim);
        println!("        {text}");
    }
    println!("\n  one source, so each lands at UNKNOWN and waits for a dig. counting");
    println!("  bots is suggestive; it is not evidence twice over.");

    let thin = CENSUS.iter().filter(|b| !b.distinct).count();
    println!("\n  {thin} of {} have a hook they share with somebody else, and that is", CENSUS.len());
    println!("  fine. A world with {} named personalities has no crowd in it,", CENSUS.len());
    println!("  and the crowd is what makes the named ones legible.");

    // ... but the ones that DO stand out should be reachable by name, or the
    // census is a wall of numbers with no way into it
    println!("\n  THE ONES THAT STAND OUT, on the worlds the map cares about\n");
    for home in ["earth", "cybertron", "outer_rim"] {
        let named: Vec<&Bot> = at(home).into_iter().filter(|b| b.distinct).take(3).collect();
        if named.is_empty() {
            println!("    {home:<12} none — every frame there is one of a crowd");
            continue;
        }
        for b in named {
            let looked_up = bot(b.id).expect("just listed");
            println!("    {:<12} {} ({})", home, looked_up.id, looked_up.class);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provenance::{assay_with, Standing};
    use std::collections::BTreeSet;

    #[test]
    fn the_census_matches_its_source() {
        // Baked from JSON, so it can go stale exactly the way `archive_gen.h`
        // could. Cheap structural check rather than a parser: every id here
        // must appear in the file, and the counts must agree.
        let src = std::fs::read_to_string(SOURCE_JSON)
            .unwrap_or_else(|e| panic!("cannot read {SOURCE_JSON}: {e}"));
        let in_file = src.matches("\"id\":").count();
        assert_eq!(
            in_file,
            CENSUS.len(),
            "{SOURCE_JSON} has {in_file} bots and the baked table has {}",
            CENSUS.len()
        );
        for b in CENSUS {
            assert!(src.contains(&format!("\"{}\"", b.id)), "{} is not in the source", b.id);
        }
    }

    #[test]
    fn every_home_is_a_real_place() {
        // a bot placed on a world the route does not have is a bot nobody can
        // ever meet
        use crate::patrol::ROUTE;
        let mut missing: Vec<&str> = CENSUS
            .iter()
            .map(|b| b.home)
            .filter(|h| !ROUTE.iter().any(|d| d.id == *h))
            .collect();
        missing.sort_unstable();
        missing.dedup();
        assert!(
            missing.len() <= 2,
            "these homes are not on the route at all: {missing:?}"
        );
    }

    #[test]
    fn the_unrecorded_shapes_are_all_in_one_place_and_all_wrong() {
        // the census fact that carries the most weight, asserted so a
        // reshuffle of the placement rules cannot quietly dissolve it
        let g: Vec<&Bot> = CENSUS.iter().filter(|b| b.class == "grimorph").collect();
        assert!(g.len() > 20, "only {} unrecorded shapes", g.len());
        assert!(g.iter().all(|b| b.home == "cybertron"), "they scattered");
        assert!(g.iter().all(|b| b.faction == "Corrupted"), "some are fine");
        // and nothing else on Cybertron is anything but corrupted
        assert_eq!(monoculture("cybertron"), Some("Corrupted"));
    }

    #[test]
    fn everything_on_earth_could_be_parked() {
        // concealment as a demographic fact
        let e = at("earth");
        assert!(e.len() > 20);
        assert_eq!(monoculture("earth"), Some("Coalition"));
        let classes: BTreeSet<&str> = e.iter().map(|b| b.class).collect();
        for c in &classes {
            assert!(
                ["car", "truck", "racer", "heli", "moto", "bus", "van"].contains(c),
                "a {c} is parked on Earth in plain sight"
            );
        }
        assert!(!classes.contains("grimorph"), "an unrecorded shape is walking around Earth");
        assert!(!classes.contains("spaceship"));
    }

    #[test]
    fn the_census_reaches_the_map_but_cannot_prove_anything() {
        let priors = census_findings();
        assert_eq!(priors.len(), 2, "the two population facts should both land");
        assert!(priors.iter().all(|p| p.source == SOURCE_CENSUS), "it became two witnesses");
        assert!(
            priors.iter().all(|p| p.bearing == Bearing::Supports),
            "a headcount is an observation, not hearsay"
        );
        for p in &priors {
            assert!(crate::archive::claim(p.claim).is_some(), "unknown claim {}", p.claim);
        }
        // one source: everything it touches lands at UNKNOWN and stops
        let a = assay_with(
            crate::archive::CLAIMS,
            crate::archive::FINDINGS,
            &BTreeSet::new(),
            &priors,
        );
        for p in &priors {
            assert_eq!(
                a[p.claim].standing(),
                Standing::Unknown,
                "'{}' was proved by counting",
                p.claim
            );
            assert_eq!(a[p.claim].support(), 1);
        }
    }

    #[test]
    fn a_census_combines_with_a_dig_rather_than_replacing_it() {
        // the point of it being Supports rather than Mentions: unlike Pale
        // Vector's testimony, a headcount COUNTS — it just cannot count twice
        let dig: BTreeSet<String> = ["substrate_sample"].iter().map(|s| s.to_string()).collect();
        let with = assay_with(
            crate::archive::CLAIMS,
            crate::archive::FINDINGS,
            &dig,
            &census_findings(),
        );
        let without =
            assay_with(crate::archive::CLAIMS, crate::archive::FINDINGS, &dig, &[]);
        assert!(
            with["architecture_is_machine"].support()
                > without["architecture_is_machine"].support(),
            "the census added nothing to a real find"
        );
    }

    #[test]
    fn most_bots_share_a_hook_and_that_is_the_design() {
        // stated as a test because the tempting fix is to write 178 more
        // hooks, and a world with 259 named personalities has no crowd in it
        let distinct = CENSUS.iter().filter(|b| b.distinct).count();
        assert!(distinct > 40, "nobody stands out at all");
        assert!(
            distinct * 2 < CENSUS.len(),
            "{distinct} of {} are individuals; where is the crowd?",
            CENSUS.len()
        );
    }

    #[test]
    fn the_roster_and_the_map_share_a_geography() {
        // the census is only worth reading because bots live where findings
        // are; if the two used different worlds there would be nothing to say
        let homes: BTreeSet<&str> = CENSUS.iter().map(|b| b.home).collect();
        let sites: BTreeSet<&str> =
            crate::archive::FINDINGS.iter().map(|f| f.site).collect();
        let shared = homes.intersection(&sites).count();
        assert!(shared >= 6, "only {shared} worlds have both bots and evidence");
    }
}
