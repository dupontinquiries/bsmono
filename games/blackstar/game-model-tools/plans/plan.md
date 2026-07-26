# overview

```
1008M   ┌── cities                                               │█       │   2%
 1.5G   │   ┌── flyer missile human.zip                          │█       │   3%
 1.6G   │ ┌─┴ missiles                                           │█       │   3%
 3.0G   ├─┴ weapons                                              │█       │   6%
 3.1G   ├── raid-bosses                                          │█       │   6%
 4.1G   ├── references                                           │█       │   8%
 5.5G   ├── terrains                                             │█       │  11%
 1.2G   │ ┌── patrol-bots                                        │█       │   3%
 1.4G   │ ├── animals                                            │█       │   3%
 6.1G   ├─┴ bots                                                 │█       │  13%
 1.1G   │ ┌── boats                                              │█░░░    │   2%
 1.2G   │ ├── hoverers                                           │█░░░    │   3%
 2.1G   │ ├── cars                                               │█░░░    │   4%
 3.0G   │ ├── spaceships                                         │█░░░    │   6%
 4.7G   │ ├── tanks                                              │█░░░    │  10%
 992M   │ │ ┌── flyer--doomlift.zip                              │█▒░░    │   2%
 1.8G   │ │ ├── flyerF-117 Nighthawk.zip                         │█▒░░    │   4%
 1.9G   │ │ ├── flyer--Sea Harrier FA2 with AIM-9L & AIM-120B.zip│█▒░░    │   4%
  10G   │ ├─┴ flyers                                             │██░░    │  22%
  24G   ├─┴ vehicles                                             │████    │  49%
  48G ┌─┴ .                                                      │███████ │ 100%
```

so yeah I was able to find more great models that have the forms of cars and planes, etc for the bots and we need to synthesize characters that could be in different regions in the patrol mode with most of them since they aren't in the script... maybe some we could weave into the campaign but I think most will end up going into patrol and raid locations... raid locations will be scattered throughout the planets... mostly in areas that are very far outside the solar system since I don't want to clutter it too much and we have the space bridges and eventual warp jump to get to those locations... some of which will be very ancient and have cybertronian aliens long divergent from the ones we know on cybertron from the lore backbone of traditional transformers

here are all the files

we need to update our python helpers to extract them and trim the tris and compress the textures as a preprocessing method. then we need to convert the `.obj` files into `.iqm` files or update the game engine to handle the latter. then we need to organize everything a bit better because my download workspace is a bit messy but you cannot help with that until the tens of GBs of data is compressed well enough to upload. the names of the zip files are important for the organization because they contain some classification metadata on what each group is so we need to route that to a rich metadata system that will elaborate all the needed details per bot.

```
.
├── ben-ten
│   └── omnitrix.zip
├── bots
│   ├── animals
│   │   ├── animal-bot--dimstreak.zip
│   │   ├── animal-bot--runningwind.zip
│   │   ├── dragons
│   │   │   └── dragonflyer--ironforge-aka Cyber Dragon Warrior bot with hand and chest beam blaster.zip
│   │   ├── extra
│   │   │   └── Quadruped Mech walker.zip
│   │   ├── falcons
│   │   │   ├── hawk-flyer--asonia.zip
│   │   │   ├── hawk-flyer--falcontip-aka hawk-mech Osprey X-80.zip
│   │   │   └── planning.md
│   │   └── spiders
│   │       ├── bot--drillsling.zip
│   │       ├── bot--slingshot.zip
│   │       ├── spiderbot--nullbite.zip
│   │       ├── spiderbot--radiachnor.zip
│   │       ├── spiderbot--rollout-aka-good detailing for realism.zip
│   │       ├── spiderbot--widowback-aka Sci-Fi Spider metal PBR low-poly game ready 3D m.zip
│   │       ├── Spider Drone V6 Cybertech.zip
│   │       └── Spider Tank.zip
│   ├── animated idle Transformers_ Fall of Cybertron Soundwave.zip
│   ├── animated - Transformers War For Cybertron Megatron.zip
│   ├── blackstar-characters
│   ├── both--set of 5--Transformers Fall Of Cybertron Combaticons.zip
│   ├── both-Slipstream_War of Cybertron.zip
│   ├── both-TRANSFORMER BULLDOG.zip
│   ├── both-Transformers Fall Of Cybertron 3 Insecticons.zip
│   ├── both-transformers fall of cybertron AirRaid.zip
│   ├── both-transformers fall of cybertron Demolishor.zip
│   ├── both-Transformers Fall Of Cybertron Grimlock.zip
│   ├── both-Transformers_ Fall of Cybertron Ironhide.zip
│   ├── both-transformers fall of cybertron Jetfire.zip
│   ├── both-Transformers Fall of Cybertron Ratchet.zip
│   ├── both-Transformers Fall Of Cybertron Scattershot.zip
│   ├── both-Transformers Fall Of Cybertron Shockwave.zip
│   ├── both-Transformers Fall Of Cybertron Slug.zip
│   ├── both-Transformers Fall Of Cybertron Soundwave.zip
│   ├── both-Transformers Fall Of Cybertron Swoop.zip
│   ├── both-Transformers Fall Of Cybertron Warpath.zip
│   ├── both-TRANSFORMERS MIXMASTER.zip
│   ├── both+weapon-Transformers Fall Of Cybertron Megatron.zip
│   ├── both--with-transform-animation--Transformers_ Fall of Cybertron Megatron.zip
│   ├── detailed-shockwave_decepticon_transformers.zip
│   ├── dinos
│   │   └── dinobot--prowler.zip
│   ├── Elita One.zip
│   ├── hq Shockwave Transformers Human Alliance.zip
│   ├── Jetfire ROTF Game.zip
│   ├── pack- decepticon soldiers s Transformers War For Cybertron Soldiers.zip
│   ├── patrol-bots
│   │   ├── bot--dragonstrike.zip
│   │   ├── bot--lucifer.zip
│   │   ├── bot--sazabi.zip
│   │   ├── bot--uplink.zip
│   │   └── bot--yukon-brawler.zip
│   ├── reference--dragon bot body-aka Sci-fi Dragon Armor.zip
│   ├── ROTF Breakaway.zip
│   ├── Sideswipe (Transformers).zip
│   ├── TRANSFORMER JETFIRE.zip
│   ├── Transformers Fall Of Cybertron Breakdown.zip
│   ├── Transformers Fall Of Cybertron Deadend.zip
│   ├── Transformers Fall Of Cybertron Dragstrip.zip
│   ├── Transformers Fall Of Cybertron flying soldier.zip
│   ├── Transformers Fall of Cybertron Hound.zip
│   ├── transformers_fall_of_cybertron_megatron-3forms.zip
│   ├── Transformers Fall Of Cybertron Menasor.zip
│   ├── Transformers fall of cybertron Motormaster.zip
│   ├── transformers_fall_of_cybertron_optimus_prime-animated.zip
│   ├── transformers fall of cybertron Perceptor.zip
│   ├── Transformers Fall Of Cybertron Sludge.zip
│   ├── Transformers Fall Of Cybertron Snarl.zip
│   ├── transformers_fall_of_cybertron_soldiers.zip
│   ├── transformers fall of cybertron soldier.zip
│   ├── transformers_fall_of_cybertron_starscream-2-forms.zip
│   ├── Transformers Fall Of Cybertron Ultra Magnus.zip
│   ├── TRANSFORMERS JOLT VOL.2.0.zip
│   ├── Transformers Revenge Of The Fallen Game Ratchet.zip
│   ├── Transformers_ War For Cybertron Bumblebee.zip
│   ├── Trypticon.zip
│   ├── vehicles
│   │   ├── Brawl Tank Transformers Human Alliance.zip
│   │   ├── Fall of CyberTron_ Bumble Bee Vehicle.zip
│   │   ├── Fall of CyberTron_ DLC G1 Optimus Prime Alt Mode.zip
│   │   ├── Fall of CyberTron_ Optimus Prime CyberTruck.zip
│   │   ├── Megatron Tank.zip
│   │   ├── PlayStation 3 ROTF Megatron.zip
│   │   ├── Transformers Fall Of Cybertron Barricade.zip
│   │   ├── Transformers Fall Of Cybertron Cliffjumper.zip
│   │   ├── transformers_fall_of_cybertron_nemesis.zip
│   │   ├── transformers fall of cybertron Perceptor.zip
│   │   ├── transformes fall of cybertron Motormaster VH.zip
│   │   ├── Trasformers Fall Of Cybertron Sideswipe.zip
│   │   └── War For CyberTron_ Bumble Bee Vehicle.zip
│   └── War for cybertron omega.zip
├── cities
│   ├── Beverly Hills, California, 323 N Rodeo Drive.zip
│   ├── brutalist_building-2.zip
│   ├── brutalist_building.zip
│   ├── city_street.zip
│   ├── detailed_old_nyc_building.zip
│   ├── low_poly_classical_new_york_city_homes.zip
│   ├── low_poly_construction_assets.zip
│   ├── low_poly_night_city_building_skyline.zip
│   ├── Mosaic B1 (CS-assets).zip
│   ├── nyc_the_high_bridge.zip
│   ├── ruined_city_free.zip
│   ├── sci-fi_mega_castle_by_jungle_jim.zip
│   └── Two California Plaza -  Los Angeles.zip
├── doodads
│   ├── car_box.zip
│   ├── door_metal_door_window.zip
│   ├── fence_protective_metal_17mb.zip
│   ├── metal_desk.zip
│   ├── metal_grate.zip
│   ├── metal_shelf_-_14mb.zip
│   ├── metal_toolbox.zip
│   ├── old_wooden_barn_house_4.zip
│   ├── old_wooden_watchtower_house_3.zip
│   └── pipe_box_metal_-_7mb.zip
├── humanoids
│   ├── call_of_duty_mw2019_-_venerated_mil-sim.zip
│   ├── exo-armor_plunger_cameraman_monsterup.zip
│   ├── halo_04.zip
│   └── humanoid--Realistic Old Russian Guy (animated).zip
├── insecticons
│   └── insecticon-vehicle-mode.zip
├── mechs
│   ├── as-is
│   │   └── mech--void.zip
│   ├── bot--T4B tank tread mech vehicle form.zip
│   ├── exo-armor_monsterup.zip
│   ├── idk
│   │   └── Mech Constructor_ Heavy Spider (Animated).zip
│   ├── le_epic_mech.zip
│   ├── PAQ Quadruped Mech.zip
│   └── planning-mechs.md
├── planets
│   ├── Mars.zip
│   └── Moon.zip
├── plan.md
├── raid-areas
│   └── not sure general ideas
│       ├── Krazoa Palace.zip
│       ├── Krazoa Shrine (Test of Observation).zip
│       └── SciFi-Corridor (Sky Palace).zip
├── raid-bosses
│   ├── astro_armor_titan_speakerman.zip
│   ├── devastator-good-for-raid-jhiaxus.zip
│   ├── plasmas_mechagodzilla_anime_trilogy.zip
│   ├── raid-boss--cherno_alpha.zip
│   ├── raidboss--gigadrill.zip
│   ├── raidboss-hardboil.zip
│   ├── raidboss-hussar_tank.zip
│   ├── raidboss--judgementor-aka large walker tank.zip
│   ├── raidboss--primitive-mantis-mech.zip
│   ├── raidboss--pyrachnid.zip
│   ├── raidboss--scorpion commander transformer reference remove the hat find a scorpion body.zip
│   ├── raidboss-starlord-aka-human sized transformer with two hammers that are sentry guns.zip
│   ├── raidboss--Strider-Tank.zip
│   ├── raidboss--tremor.zip
│   ├── raidboss--trenchmaster.zip
│   ├── raid-boss--wuthering_waves_sigillum.zip
│   ├── seperated_titan_cameraman_3.0_armor.zip
│   ├── Sphere Robot.zip
│   └── titan_cameraman_3.0_what_if....zip
├── raid-goons
│   ├── Antitank Robot Walker V1.zip
│   ├── Enemy Drone.zip
│   ├── FK02-01.zip
│   ├── Qu4drup3d T4nk.zip
│   └── War Robot ATV SH-183.zip
├── references
│   ├── animation-reference--male_skeleton.zip
│   ├── blackstar-figure-humanoid-inspiration--doom_2099_-_marvel_rivals.zip
│   ├── blackstar-mystery-scythe-reference--harvester_of_sorrows.zip
│   ├── female human face reference.zip
│   ├── humanoid--outer-rim-military.zip
│   ├── humanoid-roboskeleton-reference--terminator_rev_9.zip
│   ├── humanoid--witwicky-suit-reference-T60pa.zip
│   ├── Ice Block.zip
│   ├── ice clone
│   │   ├── ice clone body reference.zip
│   │   └── ice clone skin reference.zip
│   ├── Ice Material.zip
│   ├── ice particle.zip
│   ├── insecticons
│   │   └── reference-good insecticon model.zip
│   ├── matrix-of-leadership
│   │   ├── Matrix_Of_Leadership_Material.001_BaseColo.png
│   │   ├── Matrix_Of_Leadership_Material.001_Metallic.png
│   │   ├── Matrix_Of_Leadership_Material.001_NormalOp.png
│   │   ├── Matrix_Of_Leadership_Material.001_Roughnes.png
│   │   ├── Matrix_Of_Leadership_Material.002_BaseColo (1).png
│   │   ├── Matrix_Of_Leadership_Material.002_BaseColo.png
│   │   ├── Matrix_Of_Leadership_Material.002_NormalOp.png
│   │   ├── Matrix_Of_Leadership_Material.002_Roughnes.png
│   │   ├── Matrix_Of_Leadership_Material.003_BaseColo.png
│   │   ├── Matrix_Of_Leadership_Material.003_Metallic.png
│   │   ├── Matrix_Of_Leadership_Material.003_NormalOp.png
│   │   ├── Matrix_Of_Leadership_Material.003_Roughnes.png
│   │   ├── Matrix_Of_Leadership_Material.005_BaseColo (1).png
│   │   ├── Matrix_Of_Leadership_Material.005_Emissive.png
│   │   ├── Matrix_Of_Leadership_Material.005_Metallic (1).png
│   │   ├── Matrix_Of_Leadership_Material.005_Metallic.png
│   │   ├── Matrix_Of_Leadership_Material.005_NormalOp (1).png
│   │   ├── Matrix_Of_Leadership_Material.005_NormalOp.png
│   │   ├── Matrix_Of_Leadership_Material.005_Roughnes (1).png
│   │   ├── Matrix_Of_Leadership_Material.005_Roughnes.png
│   │   ├── Matrix_Of_Leadership_Material.006_BaseColo.png
│   │   ├── Matrix_Of_Leadership_Material.006_Metallic.png
│   │   ├── Matrix_Of_Leadership_Material.006_NormalOp.png
│   │   ├── Matrix_Of_Leadership_Material.006_Roughnes.png
│   │   ├── Matrix_Of_Leadership_Material.007_BaseColo.png
│   │   ├── Matrix_Of_Leadership_Material.007_Metallic.png
│   │   ├── Matrix_Of_Leadership_Material.007_NormalOp.png
│   │   ├── Matrix_Of_Leadership_Material.007_Roughnes.png
│   │   ├── Matrix_Of_Leadership_Material.008_BaseColo.png
│   │   ├── Matrix_Of_Leadership_Material.008_Metallic.png
│   │   ├── Matrix_Of_Leadership_Material.008_NormalOp.png
│   │   ├── Matrix_Of_Leadership_Material.008_Roughnes.png
│   │   ├── Matrix_Of_Leadership_Material.009_BaseColo (1).png
│   │   ├── Matrix_Of_Leadership_Material.009_BaseColo.png
│   │   ├── Matrix_Of_Leadership_Material.009_Metallic.png
│   │   ├── Matrix_Of_Leadership_Material.009_NormalOp (1).png
│   │   ├── Matrix_Of_Leadership_Material.009_NormalOp.png
│   │   ├── Matrix_Of_Leadership_Material.009_Roughnes (1).png
│   │   ├── Matrix_Of_Leadership_Material.009_Roughnes.png
│   │   ├── Matrix_Of_Leadership_Material_BaseColor.png
│   │   ├── Matrix_Of_Leadership_Material_Metallic.png
│   │   ├── Matrix_Of_Leadership_Material_NormalOpenGL.png
│   │   └── Matrix_Of_Leadership_Material_Roughness.png
│   ├── parts
│   │   ├── body
│   │   │   └── Iron Hand - Prosthetic Arm.zip
│   │   ├── part--Basic Rocket Engine.zip
│   │   ├── part--Rocket_BM21-122mm.zip
│   │   ├── robotparts
│   │   │   └── Doctor Octopus Arms Model Concept.zip
│   │   └── turrets
│   │       ├── Heavy Gatling Turret.zip
│   │       ├── missile pod vehicle or ground.zip
│   │       ├── Rail turret.zip
│   │       └── Turret_01a.zip
│   ├── reference--blackstar body shape for robot style transfer 03-aka Moon Knight - Blood Moon Knight.zip
│   ├── reference--blackstar throne aka Throne.zip
│   ├── reference--bot-blindside.zip
│   ├── reference--bot-watch-engineer-vine-clone-transformer-hybrid.zip
│   ├── reference--Devastator.zip
│   ├── reference--good body model details-aka Bi-Pedal Heavy Weapons Platform Concept.zip
│   ├── reference--good mech legs for IMAX level detail-aka ZBrush 4R4 how to draw mech using insert mesh.1.zip
│   ├── reference--good model detail-aka 4-legged gunner sphere droid rolling mech.zip
│   ├── reference--human-to-bot-convert-wingclip-bot.zip
│   ├── reference--military-big-suit-outer-rim-02.zip
│   ├── reference--military-big-suit-outer-rim.zip
│   ├── reference--military-suit-outer-rim-03.zip
│   ├── reference-- player camera spectator indicator-aka non-military consumer UAV drone.zip
│   ├── reference--ship parts for transformation-aka Star Sparrow Modular Spaceship.zip
│   ├── reference--smoother design of elegance-aka Laser Turret.zip
│   ├── references--potential Jhiaxus reference not sure yet-aka MIND ALPHA.zip
│   ├── reference--terrain mountain noise example earth-aka-Mount Fuji Volcano Crater with Snow - Japan.zip
│   ├── reference--three-headed-dragon-transformer-dragon-form.zip
│   ├── terrain
│   │   └── reference--terrain ref-aka- korea jeju BUM island.zip
│   └── watch-bearer
│       └── watch-bearer body reference.zip
├── robot-passive-creatures
│   ├── Old Spider Droid.zip
│   ├── rpc--Namek Spaceship.zip
│   ├── Sci-Fi Robot Spider.zip
│   └── Spider Bot.zip
├── terrains
│   ├── Cinematic Canyon Landscape - Canyonized.zip
│   ├── large scale mountains and Glacial Lake.zip
│   ├── large scale - Sandstone Towers.zip
│   ├── Meteor Crater.zip
│   ├── noise reference--Bathymetry Hunga Tonga-Hunga Ha’apai.zip
│   ├── noise reference -- Red Canyon Landscape.zip
│   ├── reference--Environment Asset_ Grassland Hill LowPoly.zip
│   ├── reference--large scale-San Francisco Bay, California.zip
│   ├── reference--noise-Half Dome, Yosemite.zip
│   ├── reference--tactical-fortress-placement-aka-Masada Fortress - ISRAEL.zip
│   ├── reference-terrain--cliff face noise aka Torre Sul.zip
│   ├── reference-terrain--Death Valley - Terrain.zip
│   ├── reference-terrain--exoplanet desert aka Star Wars_ Binary sunset over Tatooine desert.zip
│   ├── reference-terrain--large scale aka Half Dome Terrain _ Landscape.zip
│   ├── reference-terrain--large scale mountain central with even water erosion noise reference aka Mountain Terrain _ Landscape.zip
│   ├── reference-terrain-- large scale noise aka Lama lagunar.zip
│   ├── reference-terrain--large scale river bend noise aka Horseshoe Bend,colorado,Grand Canyon,scan,map.zip
│   ├── reference-terrain-- large scale small section of Mars Landscape.zip
│   ├── reference-terrain-- medium scale - Mossy rock at rocky beach scan (II).zip
│   ├── reference-terrain--nock arch door aka Rocks Set 3.zip
│   ├── reference-terrain-- noise reference aka Serra Do Segredo - Fazenda Pedra Furada.zip
│   ├── reference-terrain--rock cluster large scale - Falkenstein Table Mountain.zip
│   ├── reference-terrain--rock enclosure aka Rocks set2.zip
│   ├── reference-terrain--small scale water cave noise aka Forno da Orca.zip
│   ├── stone and water texture mapping example -- Leavitt Creek, Sonora Pass California.zip
│   ├── the_landscape_is_a_forest_in_the_mountains.zip
│   └── water erosion and landscape noise - Iceland Landscape.zip
├── turrets
│   ├── planetary Railgun Turret.zip
│   ├── RL-TRR0095213 HP.zip
│   ├── Sci-fi turret.zip
│   ├── Sci-Fi Turret.zip
│   └── Turret.zip
├── vehicles
│   ├── boats
│   │   ├── boat--MISTRAL CLASS LHD.zip
│   │   ├── boat--Super Portaviones.zip
│   │   └── Sentoku I-400.zip
│   ├── call_of_duty_advanced_warfare_-_ec-635.zip
│   ├── call_of_duty_advanced_warfare_-_gaz_cleaner.zip
│   ├── call_of_duty_advanced_warfare_-_razorback_vtol.zip
│   ├── cars
│   │   ├── 1967 Chevrolet Camaro RS 327 V-8.zip
│   │   ├── car-1968_alfa_romeo_33_stradale.zip
│   │   ├── car-1970_alfa_romeo_montreal.zip
│   │   ├── car-1970_dodge_charger_rt_fast__furious.zip
│   │   ├── car-1970_plymouth_barracuda_440-6.zip
│   │   ├── car--1977 Renault Alpine A442.zip
│   │   ├── car-2019_porsche_718_boxster_t.zip
│   │   ├── car-2024_lbsilhouette_works_murcielago_gt_evo.zip
│   │   ├── car-2026_ferrari_849_testarossa.zip
│   │   ├── car-alfa_romeo_disco_volante_speziale_coupe.zip
│   │   ├── car-alfa_romeo_disco_volante_speziale_hardtop.zip
│   │   ├── car--Bac mono.zip
│   │   ├── car--bruiser-aka-special ops racer.zip
│   │   ├── car-ford_falcon_gt_1973.zip
│   │   ├── car-hot_wheels_twin_mill.zip
│   │   ├── car-hunter_manhattan_custom.zip
│   │   ├── car-lynx_crabronix_dash_v.zip
│   │   ├── car-mad_max_-_desert_rust.zip
│   │   ├── car-madmax_sports_car.zip
│   │   ├── car-orion_skylark_gt.zip
│   │   ├── car--outback-aka jeep scifi _LAV-D3_.zip
│   │   ├── car--prowler.zip
│   │   ├── car-racing-2004_ferrari_f2004.zip
│   │   ├── car--rumbleshock-aka Futuristic Car Rigged.zip
│   │   ├── car-speed_racers_mach_6.zip
│   │   ├── car-widowmaker_-_death_race_2000.zip
│   │   ├── racer--cryofin-aka-slick mobile.zip
│   │   ├── racer--grit-aka-rocket car.zip
│   │   ├── racer--nightvent.zip
│   │   ├── racer--quickfire.zip
│   │   ├── racer--reverser-aka wasteland car with gatling gun out the back.zip
│   │   └── renault sport r.s. 01.zip
│   ├── flyers
│   │   ├── flyer--2 modes - viper.zip
│   │   ├── flyer--alphastar.zip
│   │   ├── flyer--argentglide-aka S-ER.zip
│   │   ├── flyer--batwing-aka Axis Fighter Plane - Wolf. New Order.zip
│   │   ├── flyer--bulwark.zip
│   │   ├── flyer--C-130J Super Hercules.zip
│   │   ├── flyer--cool-futuristic-flyer.zip
│   │   ├── flyer--darkguard.zip
│   │   ├── flyer--dawnshade.zip
│   │   ├── flyer--deathstroke.zip
│   │   ├── flyer-decepticon potential SpaceShip 4.zip
│   │   ├── flyer--deepspacezip
│   │   ├── flyer--doomlift.zip
│   │   ├── flyer--downdraft-aka stealth fighter.zip
│   │   ├── flyer-einzwht.zip
│   │   ├── flyerF-117 Nighthawk.zip
│   │   ├── flyer--F-14B Tomcat with AIM-54 Phoenix.zip
│   │   ├── flyer--folley.zip
│   │   ├── flyer--forlorn-aka AV-8B (NA) _PROTOTYPE_.zip
│   │   ├── flyer--freighter.zip
│   │   ├── flyer--hexaust.zip
│   │   ├── flyer-hull.zip
│   │   ├── flyer--influx-aka MG Harrier Defender 121-116-00.zip
│   │   ├── flyer--interceptor.zip
│   │   ├── flyer--intervention.zip
│   │   ├── flyer--kfir c10.zip
│   │   ├── flyer-lim-6bis_poland.zip
│   │   ├── flyer--lonestar-aka bronze space age Combat Jet Animation.zip
│   │   ├── flyer-longsword_precursors_legacy.zip
│   │   ├── flyer-mikoyan_mig-25pd_ussr.zip
│   │   ├── flyer--needlenose-aka Chinese Chengdu J-20.zip
│   │   ├── flyer--nimbus.zip
│   │   ├── flyer-pelican.zip
│   │   ├── flyer--perception.zip
│   │   ├── flyer--planetside.zip
│   │   ├── flyer--prowler.zip
│   │   ├── flyer-ps1low_poly_f-16_fighter_jet_w_animations.zip
│   │   ├── flyer-pursuer.zip
│   │   ├── flyer--red-dot.zip
│   │   ├── flyer--resonator.zip
│   │   ├── flyer--Sea Harrier FA2 with AIM-9L & AIM-120B.zip
│   │   ├── flyer--shadowscream.zip
│   │   ├── flyer--skynok.zip
│   │   ├── flyer--skyraid.zip
│   │   ├── flyer--skytower.zip
│   │   ├── flyer--soundslipzip
│   │   ├── flyer--SPACESHIP UFO 2.zip
│   │   ├── flyer-sparrowhawk.zip
│   │   ├── flyer--speedrunner.zip
│   │   ├── flyer--splinter.zip
│   │   ├── flyer--splitwing.zip
│   │   ├── flyer--tercel.zip
│   │   ├── flyer--updraft.zip
│   │   ├── flyer-valkyrie.zip
│   │   ├── flyer--verisimilitude.zip
│   │   ├── flyer-vertigo.zip
│   │   ├── flyer--wayside.zip
│   │   ├── flyer--whisperwind-aka F-117 Nighthawk (USA).zip
│   │   ├── flyer--white-tiger.zip
│   │   ├── helicopters
│   │   │   ├── flyer--ventride.zip
│   │   │   ├── helicopter--apparition-aka Boeing-Sikorsky RAH-66 Comanche.zip
│   │   │   ├── heli--eclipse-aka Z-10AG Attack Helicopter.zip
│   │   │   └── heli--stiletto-aka Future Drone 1.zip
│   │   ├── internal_ground_ao_texture.jpeg
│   │   ├── planning.md
│   │   ├── slow-flyers
│   │   │   └── slowflyer--skywalker-aka iron ufo.zip
│   │   ├── Spaceship001_Bump.png
│   │   ├── Spaceship001_Rough.png
│   │   └── spaceship01
│   │       └── Spaceship001_Dif.png
│   ├── hoverers
│   │   ├── hoverer-a8_glider_hovercar.zip
│   │   ├── hoverer--akimbo-aka Sleek Cyberpunk Car.zip
│   │   ├── hoverer--analog-aka-weaponless scout.zip
│   │   ├── hoverer--beetlejuice.zip
│   │   ├── hoverer--blackstar.zip
│   │   ├── hoverer--brutis-aka-hover tank.zip
│   │   ├── hoverer--bullhorn.zip
│   │   ├── hoverer--echotron-aka hover citizen car.zip
│   │   ├── hoverer--elegance.zip
│   │   ├── hoverer--folly-aka-land surfer sharp.zip
│   │   ├── hoverer--hardwall-aka Dystopian police car.zip
│   │   ├── hoverer--lineback-aka Modular Hover Racers Example.zip
│   │   ├── hoverer--lowride.zip
│   │   ├── hoverer--matris-aka-elegant hovercar.zip
│   │   ├── hoverer--pinprick-aka-missile barrage hovercar.zip
│   │   ├── hoverer--rubble-aka-hover tank scifi.zip
│   │   ├── hoverers--shellstike.zip
│   │   ├── hoverer--steelius.zip
│   │   ├── hoverer--trifang.zip
│   │   ├── hoverer--unitus-aka FREE Sci-Fi Vehicle 001 - public domain (CC0).zip
│   │   ├── hoverer--vectorzoid-aka-futuristic shielded hovercar.zip
│   │   ├── hoverer--Vulcanus.zip
│   │   ├── hovertank--tempest.zip
│   │   ├── hovertank--whistlewind-aka Hover IFV (PBR).zip
│   │   └── motorcycle-size
│   │       └── hoverer--Contingency.zip
│   ├── mechs
│   │   ├── Mech Constructor_ The Defender.zip
│   │   └── mech--tl-4.zip
│   ├── motorcycles
│   │   └── motorcycle--Kurogane Type 95.zip
│   ├── multi-vehicle--Spaceship Constructor.zip
│   ├── spaceships
│   │   ├── 3 Decepticon Starships.zip
│   │   ├── planning.md
│   │   ├── spaceship--Albatar.zip
│   │   ├── spaceship--ascendus.zip
│   │   ├── spaceship--briskfall.zip
│   │   ├── spaceship--cargo-01.zip
│   │   ├── spaceship--darkwing.zip
│   │   ├── spaceship--horizon.zip
│   │   ├── spaceship--marmot-aka ATT - Multi Troop Transport.zip
│   │   ├── spaceship--nemesis.zip
│   │   ├── spaceship--odyssey.zip
│   │   ├── spaceship--patronus black.zip
│   │   ├── spaceship--pax-wind.zip
│   │   ├── spaceship--pinnacle.zip
│   │   ├── spaceship--praetor-aka spaceship with partial interior.zip
│   │   ├── spaceship--rocket_ship.zip
│   │   ├── spaceship-small--hawkdrop-aka Spaceship (retopology and texturing).zip
│   │   ├── spaceship--supervac.zip
│   │   ├── spaceship--transformers fall of cybertron ark.zip
│   │   ├── spaceship--unionizer.zip
│   │   ├── spaceship--Unitron.zip
│   │   ├── spaceship-unsc_vorpal_blade_palomino-class_halo_zeo.zip
│   │   └── spaceship--widesky-aka 3km space carrier.zip
│   ├── swimmers
│   │   ├── swimmer--mecha_shark_rig.zip
│   │   └── swimmer--Sub Oceanic Hauler.zip
│   ├── tanks
│   │   ├── planning.md
│   │   ├── SPz Puma.zip
│   │   ├── tank--alpine doom.zip
│   │   ├── tank-ariete_c1.zip
│   │   ├── tank--blackshell-aka Cyber Tank (Futuristic_Cyberpunk).zip
│   │   ├── tank--boartail.zip
│   │   ├── tank--chokepoint-aka StarCraft inspired Siege Tank Tribute.zip
│   │   ├── tank--colossus-aka halo scorpion style tank.zip
│   │   ├── tank--crusher-aka Grob traktor.zip
│   │   ├── tank-ebr_105_mad_max_mod.zip
│   │   ├── tank--ekmos-aka EK23 TANK + GUN3.zip
│   │   ├── tank--embermane-aka Tank 02 Sci-fi LP.zip
│   │   ├── tank--Fiat2000 Italian heavy tank - parade version.zip
│   │   ├── tank--fivesix.zip
│   │   ├── tank--guzzle-aka  Military AAV.zip
│   │   ├── tank--hawkeye-aka modern T.I.K. - Battle Tank.zip
│   │   ├── tank--lockon.zip
│   │   ├── tank-lynx_6x6.zip
│   │   ├── tank--nightprowl-aka the M98A3 Behomoth Concept.zip
│   │   ├── tank--patton- aka scifitank patton.zip
│   │   ├── tank--pepperspray-aka panzerkampfwagen e-175 ausf B, _Fledermaus_.zip
│   │   ├── tank--riot-aka Anti-Riot Heavy Unit (Futuristic_Cyberpunk).zip
│   │   ├── tank--rollout-aka Sci-fi Tank.zip
│   │   ├── tank--T-35.zip
│   │   ├── tank-t-70.zip
│   │   ├── tank-the_russian_bmd-4-based_rakushka.zip
│   │   └── tank--vanquish-aka T-666 Concept Tank.zip
│   └── trucks
│       ├── truck-2003_peugeot_hoggar_concept.zip
│       ├── truck--barrage-aka-M270 Multiple Launch Rocket System.zip
│       ├── truck--BTR-80.zip
│       ├── truck--dino-truck-tonton.zip
│       ├── truck-giant_elephant.zip
│       ├── truck-lynx_dominante_av_445.zip
│       ├── truck-lynx_nemoxus_twin_v8.zip
│       ├── truck-mad_car_8k_resolution.zip
│       ├── truck-maz-543.zip
│       └── truck-utilitarian_vehicle.zip
└── weapons
    ├── benelli_m3_tactical.zip
    ├── blaster-references
    │   ├── Anime scifi blaster 1.zip
    │   ├── Blaster concept.zip
    │   ├── BRT-BLASTER v1 — Optimized Game-Ready Weapon.zip
    │   ├── Dc-17m Blaster.zip
    │   ├── Futuristic Handheld Blaster _ Blaster Pistol.zip
    │   ├── low-poly AKDAS SA-9.zip
    │   ├── low-poly PDR-C.zip
    │   ├── MI-214 Plasma Blaster.zip
    │   ├── Retro Sci-Fi Blaster.zip
    │   ├── Scifi Assault Rifle.zip
    │   └── Scifi Battle gun.zip
    ├── blaster weapon part or jet turbine transform.zip
    ├── bumblebee_weapon_pack.zip
    ├── hammers
    │   ├── AX - Solitude Ember - 3D Model Stylized.zip
    │   └── Halo Gravity Hammer.zip
    ├── missiles
    │   ├── flyer missile human.zip
    │   ├── missile--energy_crystal_aether_rod.zip
    │   ├── missile--piercer.zip
    │   ├── missile--Sci-fi Rocket missile 07 (Heavy).zip
    │   ├── missile--seeker.zip
    │   └── missile--thunderpod.zip
    ├── swords
    │   ├── Ancient Bronze Sword Liptov.zip
    │   ├── Caiman Rail-Slasher - 3D Model Stylized Weapon.zip
    │   ├── Caliburn_Excalibur.zip
    │   ├── Titanium Great Sword.zip
    │   └── Zweihander.zip
    └── Ultra Magnus Hammer from Transformers.zip

57 directories, 497 files

```

file contents:

```
Archive contains:
  Sideparts1_albedo.png
  Sideparts1_roughness.png
  Sideparts1_normal.png
  Sideparts1_metallic.png
  internal_ground_ao_texture.png
  Host_Madsen_Henrik_Final_Batmobile_01_render_Bolts_AO.png
  Tires1_AO.png
  chrome_AO.png
  Sideparts1_AO.png
  Body1_AO.png
  Windows_AO.png
  turbine2_AO.png
  Turbine1_AO.png
  Flame_albedo.png
  Flame_normal.png
  Flame_emissive.png
  Body1_albedo.png
  Body1_roughness.png
  Body1_normal.png
  Body1_metallic.png
  Turbine1_albedo.png
  Turbine1_roughness.png
  Turbine1_normal.png
  Turbine1_metallic.png
  turbine2_albedo.png
  Body2_roughness.png
  turbine2_normal.png
  turbine2_metallic.png
  Host_Madsen_Henrik_Final_Batmobile_01_render_Bolts_albedo.png
  Host_Madsen_Henrik_Final_Batmobile_01_render_Bolts_roughness.png
  model_0.obj
  model_1.obj
  model_2.obj
  model_3.obj
  model_4.obj
  model_5.obj
  model_6.obj
  model_7.obj
  model_8.obj
  model_9.obj
Total 40 entries (104142285 bytes)
```

# organization
the files are for structures, terrain noise, humans, transformer bots, raid areas, raid bosses, grunts, weapons, missiles, body parts, etc.

## vehicle types

there are a few basic vehicle types:
- hoverer (flies but only above the ground and is good for certain terrains that would otherwise cripple non-robot traversal)
- mech (not quite classic arms and legs more fight focused form)
- car
  - racer (car variant like bumblebee from war for cybertron behaviour)
- flyer/flier
  - helicopter
- truck
- tank
  - hovertank
  - treaded tank
- spaceship

the model designs I chose to capture a wide array of various character types and styles with uniqueness per bot and this should help us generate a wide variety of output characters by heavily leaning on the model data but also making tweaks

## creating bot forms from vehicle forms
I found lots of good inspiration to use and I want the player expression to be very rich so I want all the models to serve as subclasses of vehicle types with different styles of gunplay and movement, but the overall detail and texturing is going to have to be bumped to the level of detail that the existing models for shockwave, brawler, and scorponock have. this shouldn't be too hard as long as we make a procedural rig system for generating bot forms from lower quality models. this will also help us with LODs later on because we will need them to not lose FPS.

so we start with the bodies of those and apply the trasnformation guidelines also described in this document and we should be able to write code processor systems to interpret robot forms from moving the model parts around and slightly modifying them to better fit the scoring system that our shockwave and brawler models give us. once we have that we can try to optimize it for each vehicle class and have a look at how the files play in game. if it still looks bad we can use the human body reference skeleton and more of the high quality models or even bot pairs that we have already to improve the generative toolset. worst case scenario is that doesn't work and we have to write a small neural network GAN + DIS with the model pairings we have or with a high-quality pairing as reference and do it part-wise across the body of brawl since he is the highest detail with both a vehicle and a robot form already modeled. `brawl_robot_transformers_human_alliance.zip` and `brawl_tank_transformers_human_alliance.zip` 

then once we have the baseline we need an LOD generator so that we can step down to save on GPU compute and boost FPS when many bots are far away but still in the scene and not in the ADS view.


## filenames significance
==> ./vehicles/tanks/planning.md <==
for the names the bots are usually named tank--{botname}- model inspiration

for the other categories it is similar

dinobot--prowler => a dinobot that is named Prowler: he is going to behave in a way that relates to his vehicle form and name, and he is going to find his home in space somewhere that fits his mode choice

flyer--vertigo => a flying jet bot that is named Vertigo: they will have their own style and flying and ship design, etc.

hoverer--akimbo-aka Sleek Cyberpunk Car => a hovercar bot named Akimbo that is more sleek and cyberpunk than other models

hoverer--hardwall-aka Dystopian police car => a hovercar bot named Hardwall that is a flying police car. he obviously liked barricades form or saw something useful in the police car like visibility and speed blended with a slightly enforcer capability so maybe he takes prisoners of humans who wander out into the outer rim on smaller research missions, or maybe he was on earth and was an ally to the humans and believes in law and order... hard to say because hardwall sounds more like a decepticon name... this is where we need to fill in the lore of the world and the patrol spaces across the galaxy so that everything feels believable.

some filenames do not have much info on story or character and need extra creativity. some of these exceptions are the cars sections (cars-) which I didn't have much inspiration for yet and they might be reserved for some earth locations or some fitting space locations... maybe there can be some racetracks in space somewhere that attract these types, or maybe they end up serving as reference models for more space-styled vehicles that we can generate from their unique shapes and dimensions.

## vehicle physics and parts
we need to make sure that each bot has a vehicle mode or two and can visually match among their bot and vehicle forms. this is no easy task. we need to cover unique generated transform animations based on skeletal modeling of both the unique bots and vehicle forms. we need to be able to derive/generate a lot of the bot forms from the vehicle assets that have already encoded the character and uniqueness of the bots.

the racers will all have diferent handling, suspension, tires, aerodynamics, traction, etc based on the model and where the rig detection detects tires and mass and downforce drag etc.

we can take the known properties of the cars models with a web search and then use those to create a rule-based approximation system of how the undocumented vehicles might perform... same with the airplane/flyer models that are clearly human based desings

the flyers have to have various thruster types, positions, power. they will also vary in weapons capibilities like blasters, missiles, etc.

some basic notes:

## world interaction
we need a basic aeradynamics system to calculate rough vectors for up/down left/right and forward/backward based on speed, body shape, global wind, etc.. this will apply to flyers, racers, hoverers, tanks differently

hoverer--hardwall-aka Dystopian police car

## vehicle choice tied to location
usually mechs forms are chosen in rough terrains where guarding zones is better than covering distance

==> ./vehicles/spaceships/planning.md <==
spaceships usually are huge enough to carry multiple transformers and they transform into a giant robot mode as well. they have a longer transformation time and a crew eviction system. they land nearby to loaded maps and deploy troops and usually hold back because they are big targets for groups of enemies or stealth op giveaways

racers do well on sandy dune planets, while tanks, flyers, and hoverers will do better elsewhere. you may have also seen a few ocean swimmers and boats that would do well on planets with liquids or water-based exoplanets beyond the solar system accessible via the space bridge.

## animations
==> ./vehicles/flyers/planning.md <==
lonestar has folding wings, rotating jets, and folding landing gear

I'm not sure whether the animations came with the files I downloaded or whether there are labels on the parts that move or the joint areas

```
➜  models zip -sf 'round-2-extras/vehicles/flyers/flyer--lonestar-aka bronze space age Combat Jet Animation.zip'
Archive contains:
  Aircraft_C.png
  Aircraft_N.png
  Aircraft_S.png
  model_0.obj
  model_1.obj
  model_2.obj
  model_3.obj
  model_4.obj
  model_5.obj
Total 9 entries (40022375 bytes)
```

## complex transformations
`round-2-extras/vehicles/tanks/tank--chokepoint-aka StarCraft inspired Siege Tank Tribute.zip`
for the bot chokepoint, he should have two modes that you toggle through with the transform key so robot--(single tap transform button)->rolling form--(singletap)->siege form--(single)->rolling or --(double)->robot

this will require analyzing his model and others systematically for multi-form capabilities. some classes I can already think of are:

- racers that can hover with their tires turned downwards
- flyers that can VTOL
  - flyers with rotating rockets or turbines
  - flyers with folding wings on that can hover
- tanks that can arch their treads to increase accuracy at range
- spaceships that can load or fly

## Algorithmic transformations
some input on transformations programatically:

```
describe some general rules about how a transformer T-Pose should be folded into a vehicle based on its form and components and include details on how the two forms relate each other in detail for an animator and part modeler 

A useful way to think about Transformer design is that the robot and the vehicle are not separate designs. They are two valid folded states of a single machine. Every panel, actuator, and joint should have a purpose in both modes.

When professional designers create convincing transformations, they usually optimize for five things simultaneously:

recognizable vehicle silhouette
recognizable robot silhouette
believable mechanics
part reuse
visual rhythm (where the eye expects mass to move)

Below is a design guide I'd give an animator and hard-surface modeler.

Rule 1 — Start from a neutral engineering pose

Do not start from an action pose.

Always begin from something like:

        Head

    Arm      Arm

      Torso

      Pelvis

 Leg          Leg

Every joint should be close to neutral.

This makes rotational constraints much easier.

Rule 2 — Build around the vehicle's largest volume

The largest volume almost always becomes the torso.

Examples

Car

hood
engine
cab

↓

chest
back
spine

Jet

fuselage

↓

torso

Tank

hull

↓

torso

Millennium Falcon

entire saucer

↓

entire chest/body

The torso should consume around

40–60%

of the vehicle volume.

Rule 3 — Every limb begins inside a cavity

Never "spawn" limbs.

Instead

Vehicle

 _________
|         |
| ARM BAY |
|_________|

↓

Arm unfolds.

Every arm has

shoulder cavity
elbow clearance
wrist clearance

The cavity should exist visibly in vehicle mode.

Rule 4 — Mass moves radially

Imagine circles.

Robot

        O

   O         O

O               O


Vehicle

Everything folds toward the center.

Large pieces rotate

30°

60°

90°

180°

Rarely more.

Avoid impossible rotations.

Rule 5 — Major joints stay major joints

Robot elbow

↓

vehicle hinge

Robot knee

↓

landing gear hinge

Shoulder

↓

wing pivot

Never waste a perfectly engineered joint.

Rule 6 — Skeleton folds first

Internal frame

↓

Armor follows

Never armor first.

Correct sequence

Skeleton

↓

compress

↓

lock

↓

armor closes

Rule 7 — Armor wraps around mechanics

Think crab shell.

Frame

|||||

Armor

[|||||]


Armor should never carry loads.

Frame carries loads.

Armor protects frame.

Rule 8 — Every panel has two jobs

Example

Door

Robot

Shoulder armor

Vehicle

Door

Radar dish

Robot

Shield

Vehicle

Radar

Landing gear

Robot

Heel

Vehicle

Landing gear

Nothing decorative.

Everything functional.

Rule 9 — Robot proportions are hidden in vehicle proportions

Human

Torso

Arms

Legs

Vehicle

Main hull

Side structures

Rear structures

You are hiding anatomy inside geometry.

Rule 10 — Compress empty space

Robots have huge air gaps.

Vehicles don't.

Compression sequence

robot

|     |

↓

collapse

↓

| |

Most transformation time is spent eliminating voids.

Rule 11 — Curves become shells

Rounded vehicle

↓

layered armor

Example

Falcon

Outer ring

↓

leg armor

Circular hull

↓

rib cage

Cockpit

↓

neck

Rule 12 — Flat surfaces become layered

Never one plate.

Instead

Vehicle

========

Robot

===
 ==
  ===


Panels telescope.

Rule 13 — Think in mechanical depth

Every component exists in one of three layers.

Outer

Armor

Middle

Subsystems

Inner

Skeleton

Example

Armor

=================

Hydraulics

||||||||||||||||

Skeleton

###########

Rule 14 — Large masses rotate around primary axes

Every major object gets

Primary pivot

Secondary pivot

Slide

Lock

Example

Shoulder

Rotate

↓

Slide

↓

Rotate

↓

Lock


Almost every Transformer follows

Rotate
Slide
Rotate

Rule 15 — Never rotate through occupied volume

Animator should always check

Does this part intersect?

YES

Change sequence.


Think of the model as solving a packing puzzle.

Rule 16 — Parts remember their neighbors

Every part should have

Parent

Children

Neighbors

Example

Shoulder

Parent

Torso

Children

Upper arm

Neighbors

Chest plate

Back plate


Useful for procedural animation.

Rule 17 — Center of gravity stays predictable

Vehicle

CG

Center

Robot

CG

Pelvis


Transformation should move the center smoothly.

Avoid sudden jumps.

Rule 18 — Components migrate by function

Examples

Engine

Vehicle

rear

↓

Robot

back reactor

Wheel

Vehicle

mobility

↓

Robot

calf armor

Wing

Vehicle

lift

↓

Robot

shield

Gun

Vehicle

turret

↓

Robot

forearm cannon

Rule 19 — Preserve recognizable landmarks

You should instantly identify both forms.

Example

Falcon

Keep

Cockpit

Radar dish

Engine strip

Mandibles

Circular hull

Visible somewhere in robot mode.

Likewise, Optimus Prime always keeps truck windows on the chest because they're iconic.

Rule 20 — Transformation has phases

Instead of treating it as one long animation, divide it into stages:

1. Unlock

Mechanical latches disengage, armor gaps appear, internal joints become free.

2. Expand

The skeleton unfolds to create working space. Arms, legs, neck, and spine extend from compact storage.

3. Reconfigure

Major structural components rotate, slide, telescope, and relocate. This is where the machine changes its topology.

4. Lock

Primary structural members engage, forming a rigid load-bearing skeleton for the new mode.

5. Armor

Exterior panels swing, slide, and wrap around the finished frame, closing gaps and restoring the clean silhouette.

Rule 21 — Design around a transformation graph

Instead of thinking "this arm rotates," think of every component as a node in a dependency graph.

Vehicle Root
│
├── Hull
│   ├── Left Hull Ring
│   ├── Right Hull Ring
│   └── Engine Deck
│
├── Cockpit
│
├── Left Arm
│   ├── Forearm
│   └── Hand
│
└── Left Leg
    ├── Shin
    └── Foot

Each node should define:

Parent transform
Local pivot
Allowed rotation limits
Translation rails
Collision volume
Lock state
Animation order dependencies

For example:

Chest Unlock
    ↓
Shoulders Rotate
    ↓
Arms Extend
    ↓
Hands Deploy
    ↓
Chest Armor Close

This graph-based approach is invaluable for both animators and riggers because it enables procedural transformations, avoids impossible intersections, and makes alternate transformations easier to author.

A useful mental model

Imagine the robot as a folding industrial machine, not a person wearing vehicle parts. The robot's exposed frame is the true machine; the vehicle body is simply one highly optimized arrangement of its exterior surfaces. The most convincing designs make every panel, hinge, piston, and structural beam feel inevitable in both forms, so that neither the robot nor the vehicle appears to be the "real" form—they are simply two mechanically valid configurations of the same engineered object.

give a specific example for falcon prime

For Falcon Prime, I'd actually avoid designing it like a typical Transformer where the vehicle breaks into arbitrary chunks. The Millennium Falcon has such a strong circular silhouette that the robot should feel like the Falcon unfolded, not the Falcon disassembled.

The engineering philosophy would be:

The robot is the Falcon's internal maintenance skeleton that was always inside the ship.

That single idea determines almost every design decision.

Vehicle Layout
                     SENSOR DISH

              ___________________
         ____/                   \____
      __/                           __\
     /                                 \
    /                                   \
   |                                     |
   |          MAIN REACTOR CORE          |
   |                                     |
    \                                   /
     \__                           ____/
        \_____             _______/
              \___________/

        Cockpit           Mandibles

Instead of asking:

Where do I put the robot?

Ask:

What skeleton could exist inside this hull?

Internal Skeleton

Imagine opening the Falcon like an x-ray.

          Hull Shell
    ____________________

   /                    \
  /                      \
 |   Shoulder Ring        |
 |      ________          |
 |     /        \         |
 |    | Reactor |         |
 |     \________/         |
 |                        |
 |    Spine Column        |
 |         ||             |
 |         ||             |
 |     Pelvis Ring        |
 |      /      \          |
 |     /        \         |
 | Leg Frame  Leg Frame   |
  \                      /
   \____________________/


Notice:

The robot already exists.

The hull merely hides it.

Chest

The Falcon's circular hull stays almost intact.

Instead of becoming random armor...

It literally is the chest.

SHIP

 _____________
/             \
|             |
|             |
\_____________/

↓

ROBOT

     _______
   /         \
  |  Reactor |
  |           |
   \_________/


The chest should remain almost perfectly circular.

Like a Cybertronian knight carrying a huge circular shield built into his torso.

Cockpit

Instead of becoming a leg...

Make it the neck/head.

Ship

     Cockpit

========\____

↓

Robot

      Head

       ||

   Cockpit armor


The cockpit rotates upward almost 120 degrees.

The canopy becomes:

helmet
face guard
neck armor
Radar Dish

Never hide it.

The dish is iconic.

It becomes

shoulder sensor
communications array
electronic warfare antenna

It rotates continuously.

Mandibles

These become the forearms.

Ship

==      ==

↓

Robot

Upper Arm

|

Mandible

|

Hand


Notice the taper.

The mandibles already look like giant armored forearms.

Each contains

shield generators
blasters
grappling equipment
Arms

Instead of folding outside...

They begin underneath.

Ship Bottom

 ___________________

|   Arm      Arm    |

|___________________|


Sequence

Door opens

↓

Arm slides downward

↓

Shoulder rotates

↓

Elbow unfolds

↓

Hand deploys


Nothing magical.

Everything telescopes.

Legs

This is where Falcon Prime becomes unique.

The outer ring of the Falcon is almost a perfect leg shape waiting to happen.

Imagine cutting the hull into two giant crescents.

Top View

 _________

/         \

|         |

\_________/

↓

Split

(
)


Each crescent becomes

Thigh

↓

Knee

↓

Shin

↓

Heel

↓

Foot


The curve remains visible.

Robot legs become giant armored crescents.

Almost digitigrade.

Feet

Landing gear should never disappear.

They become

heel spurs
claws
ankle stabilizers
Landing Gear

↓

Heel

↓

Toe

Engine Strip

The blue Falcon engine across the back is perfect.

Ship

===================

Robot

Backpack

==== Reactor ====


It becomes

cooling system
jump thrusters
flight pack
Spine

The Falcon has almost no vertical depth.

So the spine must telescope.

Stored

||

↓

Deploy

||||||||||


Like a camera tripod extending.

Pelvis

Use the central maintenance tunnels.

Top

 Reactor

   ||

 Tunnel

↓

Pelvis


This becomes the strongest structural element.

Everything hangs from here.

Hands

The hands should almost disappear.

Think folding excavator.

Palm

====

↓

Finger

||

↓

Fold

[]


Five fingers collapse into one narrow package.

Head

The head should stay small.

A Falcon robot shouldn't have a giant heroic head.

Think

Shockwave
Soundwave
movie Jetfire

Small.

Buried.

Protected.

Visual hierarchy

Instead of looking like Optimus

Human wearing truck

It should feel like

Ancient spacecraft

↓

Mechanical skeleton

↓

Wrapped in hull plating

The armor floats over the frame instead of replacing it.

Cybertronian skeleton

The internal frame would look something like this.

            Optical Cluster

                ○

               / \

        Shoulder Ring

     O=============O

        \         /

         \_______/

       Reactor Spine

           ||||

           ||||

      Pelvis Truss

      /           \

     /             \

 Leg Truss      Leg Truss

     ||             ||

     ||             ||

 Crescent Armor Crescent Armor

     ||             ||

   Talon Feet    Talon Feet

I would lean even further into a biomechanical aesthetic inspired by the more intricate Cybertronian designs from the live-action films. Rather than a conventional humanoid robot with armor bolted on, Falcon Prime would resemble a walking starship framework: thousands of articulated struts, rotary joints, telescoping spars, cable bundles, and nested ring mechanisms. The recognizable Falcon hull would act as a protective exoskeleton draped over an intricate lattice of moving machinery, making the transformation feel like watching a spacecraft unfold its own internal infrastructure rather than simply rearranging body panels. That approach also gives animators rich secondary motion—sliding pistons, rotating bearing rings, tensioning cables, and synchronized locking mechanisms—that sells the enormous mass and complexity of the transformation.

write a rulebook for when to pick what since you said that some shapes break the rules
Cybertronian Transformation Design Rulebook

A geometry-first guide for deciding how a robot should fold into any vehicle.

The biggest mistake in Transformer design is treating all vehicles the same. A Formula 1 car, an Apache helicopter, a Millennium Falcon, and a dump truck cannot obey identical transformation rules because their geometry, mass distribution, and function are fundamentally different.

Instead of a universal transformation, think of every vehicle as belonging to a topology class. The topology determines almost every design decision.

Rule 0 — Preserve Identity Above All

The viewer should recognize both forms instantly.

If preserving the vehicle silhouette requires breaking a humanoid proportion, preserve the vehicle.

If preserving the robot requires destroying the vehicle outline, redesign the robot.

Identity is more important than symmetry.

Rule 1 — Find the Dominant Mass

Every object has one visually dominant volume.

Examples

Car
==========
 Engine Cabin

Jet
=========================
 Fuselage

Falcon
OOOOOOOOOOOO

Tank
##########

Motorcycle
====

Spider Tank
O--O--O--O

That dominant mass almost always becomes the robot torso.

Exceptions exist.

Rule 2 — Classify the Vehicle

Every vehicle belongs primarily to one family.

A. Longitudinal

Examples

jet
train
rocket
sword
submarine
==========================

Characteristics

long axis
narrow width
front/back emphasis

Robot

head

torso

legs

Transformation strategy

compress length
fold nose
stack sections

Examples

Optimus Jetfire Starscream

B. Radial

Examples

Millennium Falcon
flying saucer
radar platform
shield drone
   _______

 /         \

|           |

 \_________/


Characteristics

circular
balanced
equal radius

Robot strategy

chest stays circular
limbs deploy from underneath
hull unwraps outward

Never try to compress the disc into a backpack.

C. Box

Examples

truck
APC
refrigerator robot
+----------+

|          |

+----------+


Characteristics

rectangular
flat sides

Robot

Chest

Shoulders

Backpack

Transformation

Panels fold.

Corners become limbs.

D. Winged

Examples

Aircraft

Bird

Dragon

\---------/


Characteristics

Huge lateral span.

Robot

Wings usually remain

wings
backpack
shields

Avoid completely hiding them.

E. Multi-leg

Examples

Spider tank

Walker

Excavator

\ | /

 O

/ | \


Robot

Keep extra limbs.

Do not force humanoid anatomy.

F. Vertical

Examples

Rocket

Tower

Mech

||

||

||


Robot

Height already exists.

Little compression required.

Rule 3 — Match Robot Skeleton

Vehicle topology chooses skeleton.

Vehicle	Skeleton
Jet	Human
Falcon	Gorilla/Crab
Tank	Heavy knight
Motorcycle	Runner
Spider tank	Multi-legged
Bird	Digitigrade

Don't force everything into Optimus Prime proportions.

Rule 4 — Preserve Large Curves

Curves are expensive.

Never destroy them.

Falcon

(

)


↓

Robot

Leg armor

Shoulders

Chest

Same curve.

Rule 5 — Preserve Long Lines

Jets

=================

↓

Robot

Legs

Back

Sword

Wing

Long lines remain long.

Rule 6 — Respect Functional Components

Every component asks

"What is my equivalent function?"

Example

Vehicle

Engine

↓

Robot

Reactor

Vehicle

Wheel

↓

Robot

Knee

Vehicle

Cockpit

↓

Robot

Head

Vehicle

Wing

↓

Robot

Shield

Rule 7 — Hide Empty Space

Large vehicles have hollow interiors.

Use them.

Ship

 ___________

|           |

|   Empty   |

|___________|


↓

Robot

Arms

Spine

Pelvis

Rule 8 — Large Masses Move Slowly

Huge components

Rotate.

Small components

Fold.

Tiny components

Articulate.

Never animate everything equally.

Rule 9 — Shell First or Skeleton First?

This depends on vehicle type.

Jets

Skeleton first.

Shell wraps.

Cars

Both simultaneously.

Falcon

Shell barely moves.

Skeleton emerges.

Huge difference.

Rule 10 — Continuous Surface Rule

Ask

Can I trace this panel with my finger?

If yes

Try to keep it continuous.

Breaking surfaces creates visual confusion.

Rule 11 — Number of Major Chunks

Large recognizable vehicles

Need fewer chunks.

Approximate guidelines:

Vehicle	Major chunks
Sedan	8–12
Jet	10–16
Semi truck	12–20
Falcon	4–8
Tank	8–14
Motorcycle	6–10

The Falcon is recognizable because of its single continuous saucer, so slicing it into dozens of unrelated pieces weakens its identity.

Rule 12 — Choose Limb Origin

Where should limbs emerge?

Use the vehicle's geometry.

Shape	Arm Origin	Leg Origin
Car	Doors	Rear axle
Jet	Intakes	Rear fuselage
Falcon	Underside center	Outer hull ring
Tank	Hull sides	Rear hull
Helicopter	Cabin	Landing skids
Spaceship	Cargo bays	Structural ribs
Rule 13 — Symmetry Rule

Perfect symmetry

↓

Robot becomes boring.

Allow

one larger shoulder
offset sensor
exposed weapon
damaged armor

Especially for Falcon Prime.

Its asymmetry is iconic.

Keep it.

Rule 14 — Mechanical Priority

Every moving component gets assigned one of these priorities:

Structural

↓

Locomotion

↓

Weapon

↓

Sensor

↓

Armor

↓

Decoration

Structural parts should never depend on decorative panels to move. Armor can wait until the frame is locked.

Rule 15 — Choose the Correct Transformation Philosophy

There are really four distinct transformation philosophies.

1. Compression

Best for:

cars
trucks

Large volumes collapse into smaller ones.

2. Expansion

Best for:

Millennium Falcon
UFOs
compact spacecraft

The robot unfolds from within the vehicle.

3. Rotation

Best for:

jets
helicopters

Large assemblies pivot around long axes.

4. Reconfiguration

Best for:

construction equipment
Cybertronian alt modes
alien vehicles

Components slide, telescope, nest, and rearrange with less emphasis on simple hinges.

Rule 16 — Preserve Mechanical Logic

Every visible part should answer three questions:

What does it do in vehicle mode?
What does it become in robot mode?
Why is it located there?

If any answer is "because it looks cool," the design probably needs another pass.

Rule 17 — Decide What Doesn't Transform

Not everything has to change role.

Some components remain nearly identical:

reactor cores
bearings
gyro assemblies
structural rings
power buses
cooling ducts

These become the anchors that make the transformation feel believable.

Falcon Prime Example

Applying the rulebook to Falcon Prime:

Vehicle Component	Rule Applied	Robot Component
Circular saucer hull	Preserve dominant mass	Chest and upper torso
Cockpit pod	Preserve function	Head and neck assembly
Radar dish	Preserve landmark	Shoulder sensor array
Outer hull ring	Preserve large curves	Crescent-shaped legs
Cargo bays	Hide empty space	Arm storage cavities
Mandibles	Preserve long lines	Forearms and shields
Engine strip	Preserve function	Back-mounted thruster pack
Landing gear	Reuse mechanisms	Feet and heel spurs
Central maintenance tunnels	Structural priority	Spine and pelvis
Internal ribs	Skeleton-first	Cybertronian load-bearing frame

The result is a robot that doesn't feel like "a person wearing the Millennium Falcon." Instead, it feels like the Falcon itself has unfolded to reveal the intricate Cybertronian machine that always existed inside its hull. That's the distinction that separates a memorable transformation from one that feels arbitrary.
```

## Misc
### Terrain Noise
there are some terrain noise examples that are for helping us to generate better noise models for alien planets and moons that are earthlike. we need to refine our systems for generating worlds because we have lots of places we will need to space out our raids.

### Raids
I have a lot of raid boss assets that we will have to create additional lore and 6-stage progressions for. each raid will be a 6 player multiplayer PvE (player vs environment) experience and there will also be friendly allies sometimes from the main game or from our bot asset pool

### Friendship System
build out a squad and take AI-controlled followers with you that move about the map on their own, but strategically much like the cover bots algorithms we had designed. The difference is that the player position is also in the mix. Sometimes the player might choose to elect a leader and follow them as their AI guides you on patrols and into combat, while other times the player may choose to be leader or designate a multiplayer joined player as the leader. This opens the door for large-scale battles across the universe in big epic space maps or amazing planets in the solar system and beyond.

### LA
there are some buildings and LA related assets for helping with the campaign missions there because the graphics are far from what is considere to be a recognizable location representing LA. Either place the assets directly or let's make systems that are good at generating believable cities on terrain.

```
➜  round-2-extras tree | rg California
│   ├── Beverly Hills, California, 323 N Rodeo Drive.zip
│   └── Two California Plaza -  Los Angeles.zip
│   ├── reference--large scale-San Francisco Bay, California.zip
│   ├── stone and water texture mapping example -- Leavitt Creek, Sonora Pass California.zip
```

### Critters and Doodads
There are some additional objects and critters to populate worlds with when appropriate that you can use as doodads or compress them down to systems that generate believable objects.

### Character Customization
Character customization will have to be expanded to include relics discoverd in raids and patrols as well as special abilities unlocked via the myriad of new vehicle forms this update will add.

### Potential Raid Ideas
Raids could lead towards the transformers helping the clones a long time ago to lock down the galaxy from the hijacks. Here are some ideas for what to do with raid bosses when designing the raids

```
generate interesting deep space transformer relic lore for each raid boss based on the name:


raid-bosses
├── astro_armor_titan_speakerman.zip
├── devastator-good-for-raid-jhiaxus.zip
├── plasmas_mechagodzilla_anime_trilogy.zip
├── raid-boss--cherno_alpha.zip
├── raidboss--gigadrill.zip
├── raidboss-hardboil.zip
├── raidboss-hussar_tank.zip
├── raidboss--judgementor-aka large walker tank.zip
├── raidboss--primitive-mantis-mech.zip
├── raidboss--pyrachnid.zip
├── raidboss--scorpion commander transformer reference remove the hat find a scorpion body.zip
├── raidboss-starlord-aka-human sized transformer with two hammers that are sentry guns.zip
├── raidboss--Strider-Tank.zip
├── raidboss--tremor.zip
├── raidboss--trenchmaster.zip
├── raid-boss--wuthering_waves_sigillum.zip
├── seperated_titan_cameraman_3.0_armor.zip
├── Sphere Robot.zip
└── titan_cameraman_3.0_what_if....zip

If these are meant to feel like ancient Cybertronian relics rather than simply "big enemies," a useful theme is that each raid boss is not merely a machine—it is an archaeological discovery. Millions of years after the Collapse, drifting fleets discover dormant war engines scattered between galaxies, each built for a purpose that no longer exists. Most are partly sentient because Cybertronian technology imprints memory into its alloys. Every boss tells part of the history of an ancient civilization.

Astro Armor Titan Speakerman
"The Last Resonance"

Classification: Stellar Communications Fortress

Long before subspace relays, entire civilizations communicated through stellar harmonics. The Titan Speakermen were walking observatories capable of vibrating moons and stars into enormous resonant transmitters.

Their voices carried through folded space.

Entire wars ended because one Titan could sing across half a galaxy.

When the Harmonic Civil War began, enemy AI infected the communication frequencies themselves.

The Speakermen became weapons.

Their sonic arrays could fracture planets by matching tectonic resonance.

Eventually they were silenced.

Now only one remains.

It still broadcasts.

Every century another fleet answers.

No fleet ever returns.

Devastator Jhiaxus
"Prototype Omega"

Jhiaxus believed individual Transformers were obsolete.

His answer:

planet-scale modular organisms.

Rather than constructing cities, he built giant Cybertronians whose organs were entire factories.

Every limb contained:

clone foundries
forge reactors
drone fabrication
nanite oceans

Destroying one only caused it to rebuild itself.

Only the destruction of Cybertron halted the project.

The prototype drifted unfinished into dark space.

It has spent four million years finishing itself.

Plasma's Mechagodzilla
"The Planet Burner"

Built around a captured stellar core fragment.

Unlike conventional Transformers, it never transformed.

Its body was the weapon.

Its spinal plasma spine could:

ignite gas giants
evaporate oceans
melt orbital rings

Ancient records say entire civilizations surrendered after seeing its dorsal fins emerge above planetary horizons.

Its reactor still burns.

The star inside refuses to die.

Cherno Alpha
"The Walking Reactor"

Created during the Radiation Crusades.

Its armor isn't armor.

It is containment.

Inside is an impossible reactor.

Every movement relieves pressure.

If it ever stops walking...

the reactor reaches criticality.

Ancient engineers intentionally removed any shutdown mechanism.

Millions of years later...

it is still marching.

Gigadrill
"The Core Piercer"

Nobody knows why planets have metallic hearts.

Gigadrill was built to find out.

It drilled through:

moons
worlds
dwarf planets

Some worlds screamed.

Others contained machinery.

Ancient logs mention structures older than Cybertron itself buried inside planetary cores.

Gigadrill disappeared after drilling into something beneath a rogue planet.

When rediscovered...

its drill was glowing with unknown symbols.

Hardboil
"The Pressure King"

Designed for gas giant warfare.

Instead of missiles...

it weaponized pressure.

It compressed atmosphere into artificial neutron-density shells.

Entire fleets vanished without explosions.

Only crushed wreckage remained.

Its internal pressure chambers still cycle.

Nearby moons experience mysterious tides whenever it awakens.

Hussar Tank
"The Charge"

Built by an order of Cybertronian knights.

Unlike modern tanks...

it accelerated continuously.

Every kilometer increased kinetic energy.

After sufficient distance...

nothing in existence could survive impact.

Records claim one Hussar destroyed an orbital ring simply by refusing to stop.

Its engines have never cooled.

Judgementor
"Executor of Empires"

Not a tank.

Not a walker.

A mobile courtroom.

Entire civilizations stood trial beneath its cathedral-sized body.

Its verdicts were simple.

Innocent.
Guilty.

"Guilty" meant extinction.

Its rail cannons were called Sentences.

Ancient civilizations still orbit as asteroid belts.

Primitive Mantis Mech
"The First Hunter"

One of the earliest Cybertronians.

Before language.

Before cities.

Before transformation became elegant.

Its nervous system evolved rather than being engineered.

Unlike modern Transformers...

it still thinks like an apex predator.

It studies prey.

Learns habits.

Returns centuries later.

Pyrachnid
"Forge Widow"

Built inside stellar furnaces.

Its web isn't silk.

It spins superconducting plasma filaments.

Entire battleships become trapped.

Then slowly melted.

Ancient forge worlds considered Pyrachnids sacred because every web also manufactured alloys unknown elsewhere.

Destroying one means losing technologies impossible to rediscover.

Scorpion Commander
"The Frontier Marshal"

Not a frontline soldier.

A colonizer.

Every conquered planet received one.

Its tail deployed:

factories
energy grids
military bases
terraformers

Every claw became a city.

Every leg became infrastructure.

Millions of abandoned colonies still await commands from a commander long presumed dead.

Starlord
"Twin Hammers"

Human-sized.

Almost overlooked.

Its weapons appear ceremonial.

Two enormous hammers.

Each hammer unfolds into an autonomous defense fortress.

The Starlord itself fights like an agile knight while the hammers independently become:

anti-air batteries
missile arrays
drone nests
sniper towers

No one knows whether the hammers protect their master...

or imprison it.

Strider Tank
"The Horizon Walker"

Designed for impossible terrain.

Mountain ranges.

Ocean trenches.

Asteroid fields.

It never required roads.

Its stride stabilization predicts gravitational fluctuations several seconds before they happen.

Ancient explorers believed if a Strider chose a destination...

it would eventually arrive.

Even if the galaxy changed around it.

Tremor
"Planetbreaker"

Not seismic.

Gravitational.

Its legs contain singularity oscillators.

Instead of shaking the ground...

it shakes spacetime.

Earthquakes become secondary effects.

Nearby satellites slowly fall from orbit.

Stars exhibit measurable wobble.

Its heartbeat is detectable light-years away.

Trenchmaster
"The Ocean King"

Created for abyssal wars.

Most Cybertronians fear deep oceans.

Trenchmaster considered them highways.

It commanded oceans using pressure resonance.

Entire continental shelves could rise.

Ancient sea civilizations still tell stories of the Iron Leviathan walking beneath them.

Some sonar recordings continue today.

Wuthering Waves Sigillum
"The Seal"

Nobody knows what it actually is.

It isn't a warrior.

It is a lock.

Ancient inscriptions repeatedly state:

"The Sigillum shall never awaken."

Naturally...

someone woke it.

Every attack it performs resembles defensive mechanisms rather than offense.

Almost as if the real enemy lies somewhere behind it.

Destroying the Sigillum may not be victory.

It may simply open the door.

Titan Cameraman 3.0 Armor
"Archivist"

The Camera Titans were never soldiers.

Originally.

They documented reality.

Every photon they captured was permanently stored in quantum crystal memory.

This Titan remembers:

Cybertron's birth
the first Prime
extinct galaxies
forgotten species

Its armor is layered memory storage.

Damaging it physically erases history.

Raid groups unknowingly destroy irreplaceable records every time they defeat it.

Sphere Robot
"The Observer"

Found drifting between galaxies.

No engines.

No weapons.

No visible sensors.

Yet it always arrives before major events.

Entire civilizations have independently recorded identical spheres.

Some have watched stars die for billions of years.

Others appear moments before dimensional catastrophes.

Every attempt to open one has failed.

Its shell predates known matter.

Nobody is certain whether it is a probe...

or an eye.

Titan Cameraman 3.0 (What If...)
"The Divergence"

Not from this timeline.

Scans reveal impossible manufacturing dates.

Its memory contains events that never happened.

Destroyed Primes who still live.

Victorious Decepticons who lost.

Worlds that never existed.

Ancient chronologists theorize it originates from a branch timeline where the Great War ended differently.

Its armor constantly phases between realities, causing pieces of alternate histories to momentarily become real during battle.

No one knows whether defeating it preserves the current timeline—or erases the only surviving witness to another one.

Hidden overarching relic mystery

Across all of these relics is engraved the same impossibly ancient glyph:

"The Ninth Forge Sleeps."

No historical database can translate it. The glyph predates Cybertronian civilization itself.

As players defeat each raid boss, they recover fragmented "Memory Shards." Individually they are meaningless, but together they reveal a chilling truth: none of these machines were the greatest weapons ever built. They were wardens, builders, archivists, judges, colonizers, communicators, and seals—parts of a vast interstellar system created to contain something buried long before the first Transformer transformed.

The final revelation is that the relics were never scattered by accident. They were deliberately positioned around the galaxy as a distributed lock. Every raid victory weakens that ancient lock, advancing a hidden campaign that culminates in the awakening of whatever the mysterious Ninth Forge was built to imprison.

I actually like where this is going much more. Instead of every raid boss being a "galaxy destroyer," make them legendary military relics. Think Halo Scarabs, Destiny raid bosses, Monster Hunter Elder Dragons, or Warframe Eidolons—dangerous because of their role and mechanics, not because every one can shatter planets.

That also makes the overarching mystery stronger. Every relic becomes a piece of an ancient military doctrine.

Relic Classification

Rather than "raid bosses", every relic belongs to an old Cybertronian order.

Type	Purpose
Vanguard Relics	Frontline assault units
Forge Relics	Mobile factories
Warden Relics	Prison guards
Pioneer Relics	Colonization
Hunter Relics	Assassination
Siege Relics	Fortress killers
Archive Relics	Historical memory
Sentinel Relics	Border patrol

No relic understands the war is over.

Astro Armor Titan Speakerman
Relic Class

Resonance Hunter

Instead of missiles...

it weaponizes sound.

Not "destroy planets."

Instead:

overload optics
disable targeting
scramble radios
stun transformation joints
reverse gyroscopes
temporarily disconnect limbs

Imagine fighting while your HUD constantly glitches.

Weapons become unusable.

Transformations fail halfway.

Your left arm suddenly belongs to someone else's control network.

Its objective is never to kill first.

It cripples entire squads.

Then allied relics arrive.

Raid

6 Players

Objective:

Destroy four resonance towers powering the armor.

Intermittent events:

sound pulses requiring cover
inverted controls
false radar contacts
duplicated audio making callouts unreliable

Final phase:

Fight the weakened Speakerman while every attack attempts to interrupt teamwork.

Communication becomes the mechanic.

Devastator Jhiaxus

I honestly wouldn't change much.

This one should remain horrifying.

Instead of "kill boss."

Players are dismantling an unfinished civilization.

Every destroyed limb simply opens another factory.

The raid slowly realizes:

they are inside the machine.

Plasma Mechagodzilla
Relic Class

Forge Beast

Rather than a skyscraper...

Imagine roughly Metroplex knee-height.

Still enormous.

Quadrupedal.

Ancient.

Its body evolved around a plasma reactor.

It doesn't transform because it never needed to.

Raid

Takes place inside a dying stellar refinery.

The beast feeds on molten plasma rivers.

Players reroute cooling systems.

Freeze legs.

Collapse bridges.

Expose reactor vents.

Final phase:

The reactor overloads.

Instead of exploding...

it enters berserk mode.

Players must escape while finishing it.

Very Monster Hunter.

(Former Cherno Alpha)

I'd rename it:

Cindermarch

or

Iron Wake

or

Ashwalker

Its story becomes:

An experimental walking containment reactor.

It cannot be shut down.

Only redirected.

Raid

Not a boss fight.

An escort.

The objective:

Lead it into an ancient Spacebridge Cannon.

Meanwhile:

waves of scavenger bots arrive hoping to consume the unstable reactor.

Raid split:

Three players

protect bait emitters.

Three players

hold bridge generators.

Mini Boss 1

Reactor parasites.

Mini Boss 2

Scavenger warlord.

Final

Activate bridge.

Force the relic through before overload.

The "kill" is environmental.

Gigadrill

I love making this one more engineering-focused.

The drill cannot simply be damaged.

It constantly repairs.

Raid:

Section 1

Escort mining crawlers.

Acquire EMP explosives.

Section 2

Mini boss

Guardian excavator.

Section 3

Ride on Gigadrill while drilling through asteroid crust.

Destroy coolant lines.

Section 4

Inside drill shaft.

Fight repair drones.

Section 5

Mini boss

Core Stabilizer.

Section 6

Boss

Now the drill overheats.

Players rotate cooling valves while exposing drill gearbox.

Feels like Deep Rock Galactic mixed with Destiny.

Hardboil

Absolutely lean into Cannonbolt.

Gigantic spherical Transformer.

Transforms by rolling.

Raid:

Asteroid belt.

The boss ricochets off asteroids.

Every impact creates debris fields.

Players:

dodge rocks
predict rebounds
bait impacts into explosive ore deposits

Final:

Destroy stabilizer gyros.

It can no longer control bouncing.

Eventually crashes.

Hussar Tank

Two possibilities.

Ancient Knight

Hover lance.

Energy shield.

Mounted cavalry.

Raid centered around jousting.

Interrupt charge attacks.

Break shield.

Countercharge.

Future Cavalry

Hypermobile hover tank.

Never stops moving.

Raid:

Players capture boost pylons.

Use speed against it.

Less health.

Very high mobility.

Judgementor

Instead of giant.

Make it roughly six stories.

A walking execution platform.

Every minute it "judges."

Marks one player.

Entire raid protects them.

If judgement completes:

instant wipe mechanic.

Destroy legal data cores to reduce verdict timer.

Very clean mechanic.

Primitive Mantis

This one has enormous potential.

Instead of one raid...

make three variants.

Hunt Variant

Dense jungle.

Mantis stalks players.

Stealth.

Sound.

Ambush.

Pack Variant

Three Alpha Mantises.

Never fight alone.

Split attention.

Pack AI.

Shepherd Variant

Short-range teleport.

Never attacks immediately.

Instead...

teleports players into kill zones.

Separates teammates.

Feels psychological.

Pyrachnid

Perfect in a forge world.

Molten foundries.

Zero gravity.

Flamethrower webs.

Burning metal strands.

Raid objective:

Collapse cooling towers.

Freeze web anchors.

Prevent reactor ignition.

Scorpion Commander

I LOVE your idea.

This feels like an RTS mission.

Team Alpha (3)

Invades factory.

Destroys:

assembly lines
reactors
command AI

Team Bravo (3)

Protects objectives.

Escort civilians.

Hold power stations.

Destroy endless drone waves.

Eventually both reunite.

Fight commander.

Fantastic raid.

Starlord

Small relic.

Extremely skilled.

Two sentry hammers.

Raid begins with exploration.

Ancient ruined observatory beyond a forgotten Spacebridge.

Boss constantly relocates.

Hammers independently:

sniper mode
mortar mode
laser fences

Eventually players steal one hammer.

Then another.

Final duel:

Boss fights unarmed...

and becomes dramatically more dangerous.

Strider Tank

This one practically designs itself.

Mountain world.

Huge valleys.

Aircraft must remain below ridge lines.

Otherwise:

AA railguns.

Ground units:

navigate canyons.

Boss:

moves atop mountains.

Players constantly relocate.

Feels like Top Gun meets Halo.

Tremor

Tone down.

Instead of gravity.

Make it a seismic engineer.

Purpose:

Collapse tunnels.

Destroy fortifications.

Raid:

Underground mining colony.

Boss keeps collapsing terrain.

Players reroute drilling machines.

Open escape routes.

Prevent cave-ins.

Destroy vibration generators.

Simple.

The Ninth Forge

Every relic carries one fragment.

Players think they're gathering relic technology.

Eventually...

they assemble a key.

The key unlocks:

The Ninth Forge.

Nobody knows what's inside.

Even the relics don't know.

They were built only to guard it.

Raid Rewards

This is where Transformers MMOs can become incredibly unique.

Instead of "Sword +17."

Everything is modular.

Ancient Components

Install relic hardware.

Examples:

Speakerman Resonator

→ Sonic pulse ability.

Gigadrill Servo

→ Faster melee drilling attacks.

Pyrachnid Heat Core

→ Flame weapons ignite terrain.

Scorpion Forge Tail

→ Deploy temporary drone factory.

Hardboil Gyroscope

→ Rolling travel form.

Transformation Parts

Every raid unlocks body pieces.

Not armor.

Actual Transformer geometry.

Examples:

New shoulders.

New doors.

New wheel designs.

New legs.

New cockpits.

New wing assemblies.

New visor.

Alternate Transformation Forms

Instead of class restrictions.

Collect relic blueprints.

Unlock:

Hover tank

Spider

Walker

Artillery carrier

Heavy hauler

Tracked siege engine

Six-legged scout

Drill transport

Beast forms

Hybrid forms

Players gradually become increasingly unique combinations of ancient Cybertronian engineering traditions.

Ancient Relic Protocols

Passive abilities learned from recovered memory shards.

Examples:

Hunter Protocol

Increased damage after remaining unseen.

Forge Protocol

Build deployables faster.

Resonance Protocol

Sonic attacks interrupt enemies.

Warden Protocol

Better shields near objectives.

Pioneer Protocol

Summon construction drones.
Memory Echoes

Rather than lore logs, defeating a relic unlocks a Memory Echo—a replayable fragment from millions of years ago, viewed through that relic's optics. These are playable historical missions rather than cutscenes, allowing players to witness key events from the ancient war and slowly piece together the truth behind the Ninth Forge. Completing Echoes rewards cosmetic variants based on the relic's original era (pristine alloy finishes, ancient insignias, prototype transformation animations) and occasionally unlocks forgotten combat techniques that modern Transformers had long since abandoned.

This progression creates a satisfying loop: each raid not only grants mechanical upgrades and new transformation parts, but also reveals another piece of the ancient puzzle. By the time players have mastered every relic, they've unknowingly reconstructed the key to the Ninth Forge—and must decide whether to preserve the ancient seal or discover why an entire civilization built dozens of legendary machines solely to keep one forgotten place locked forever.

and add more simple relics like some that add armor lock, active camo, small assistant bot factory (soundwave style), short-range blink double jumps, multiple vehicle forms (car + hover the wheels, or tank that turns into a mech to handle different terrain types), size modifications (become 3x the normal size with new forms), AoE field effects along the lines of the soundweapons and pyroweapons, etc.

I think one of the biggest missed opportunities in Transformer games is that relics shouldn't just increase stats—they should fundamentally change how your Transformer is built and played. Imagine if every relic represented a forgotten engineering discipline from ancient Cybertron. You're not looting a +5 sword; you're reverse-engineering lost technology.

I would organize them into relic families, each with a recognizable philosophy. That also gives players a reason to hunt specific raids instead of simply chasing the highest item level.

Resonance Relics

"Matter is only waiting for the right frequency."

Developed by ancient communications and electronic warfare specialists.

Resonance Core

Active pulse that disables nearby missiles and briefly interrupts enemy abilities.

Sonic Armor

A short-lived expanding sound barrier that reduces incoming projectile damage.

Echo Step

Leave behind an audio decoy that continues moving for several seconds.

Harmonic Lock

Briefly prevents nearby enemies from transforming.

Resonance Jump

Creates a downward sonic burst, effectively granting a second jump.

Seismic Pulse

Ground slam that briefly staggers enemies.

Stealth Relics

"The greatest victory is remaining unseen."

Ancient reconnaissance technology.

Active Camouflage

Nearly invisible while stationary.

Adaptive Cloak

Moving slowly maintains partial invisibility.

Ghost Engine

Vehicle form produces almost no radar signature.

Blink Drive

Short-range teleport.

Upgradeable into:

triple blink
blink while aiming
blink leaves decoy
blink explosion
Sensor Jammer

Enemies lose target lock.

Forge Relics

"Every battlefield is a factory."

These are among my favorites because they make the player feel like a mobile base.

Drone Forge

Produces one helper drone every 30 seconds.

Soundwave Core

Deploys a small companion bot that can transform independently.

Possible companions include:

scout bird
repair spider
ammo carrier
shield projector
suicide drone
sniper drone
hacking drone
Deployable Turret

Folds out from your shoulder or backpack.

Mobile Repair Station

Transforms into a temporary field workshop.

Nano Foundry

Slowly repairs nearby allies.

Mobility Relics

"No terrain should ever stop a Transformer."

Hover Wheels

Car mode now hovers several meters above ground.

Jump Jets

Double jump.

Triple Jump

Consumes energon.

Wall Boosters

Drive briefly on vertical walls.

Magnetic Wheels

Ceiling driving.

Rocket Dash

Forward burst.

Air Brake

Instantly stop while flying.

Multi-Mode Relics

One of the coolest ideas.

Instead of one alternate mode...

gain multiple.

Dual Chassis

Example

Sports Car

↓

Hover Car

↓

Robot

Siege Platform

Tank

↓

Walker Tank

↓

Robot

Aerospace Module

Jet

↓

VTOL

↓

Robot

Expedition Platform

Truck

↓

Crawler

↓

Robot

Beast Hybrid

Wolf

↓

Hover Bike

↓

Robot

Some legendary relics could even unlock four-mode Transformers.

Expansion Relics

Built using ancient mass-distribution technology.

Growth Matrix

Temporarily become

2–3×

larger.

Gain

stronger melee
heavier footsteps
increased knockback

Tradeoff

larger target.

Compression Matrix

Shrink.

Become much harder to hit.

Faster.

Less armor.

Modular Armor

Additional armor physically unfolds.

Changes silhouette.

Siege Configuration

Additional weapons deploy.

Movement slower.

Thermal Relics

Pyrachnid technology.

Flame Thrower

Classic.

Heat Field

Nearby enemies slowly overheat.

Plasma Trail

Driving leaves burning ground.

Meltdown Core

Low health causes heat bursts.

Thermal Lance

Piercing beam.

Forge Aura

Nearby allies reload faster because weapons remain heated.

Cryo Relics

Counterpart to Pyrachnid.

Freeze beam.

Ice mines.

Cooling mist.

Slow fields.

Cryo missiles.

Gravity Relics

Instead of Tremor destroying planets...

localized manipulation.

Gravity Well

Pull enemies together.

Repulsor Wave

Push everyone away.

Heavy Field

Enemies move slowly.

Low Gravity

Huge jumps.

Orbit Shield

Projectiles curve around you.

Warden Relics

Defense specialists.

Armor Lock

Halo style.

Become immobile.

Nearly invulnerable.

Reflective Barrier

Bounce projectiles.

Deployable Wall

Portable cover.

Guardian Dome

Bubble shield.

Taunt Beacon

Force AI enemies to attack it.

Hunter Relics

Primitive Mantis technology.

Predator Vision.

Track wounded enemies.

Pounce.

Wall Cloak.

Ambush Damage.

Short teleport after melee kills.

Temporal Relics

Very rare.

Recovered from Ninth Forge ruins.

Brief slow motion.

Rewind last 3 seconds.

Store damage then release.

Duplicate yourself for 5 seconds.

Prediction marker.

Shows enemy future movement.

Dimensional Relics

Late game.

Pocket inventory.

Portal grenade.

Phase through walls.

Temporary alternate dimension.

Walk through solid matter.

Ancient Beast Relics

Instead of changing weapons...

they partially rewrite your body.

Wolf Legs

Faster sprint.

Raptor Feet

Higher jumps.

Scorpion Tail

Extra melee.

Spider Limbs

Wall climbing.

Dragon Wings

Glide.

Rhino Armor

Charge attack.

Titan Fragments

Tiny pieces of Metroplex-class technology.

Mini Shoulder Cannon.

Deployable hand drone.

Titan stomp.

Energy fist.

Back-mounted missile pod.

Memory Relics

Probably my favorite concept.

They don't modify the body.

They modify the player's understanding of the battlefield.

Enemy weak points become highlighted.

See hidden relic caches.

Reveal invisible enemies.

Predict boss attacks.

Automatically translate ancient Cybertronian ruins.

Relic Synergies

The most interesting builds emerge when relics interact rather than simply stacking effects. A Resonance Core paired with Blink Drive creates a "Phase Echo" build where every teleport leaves behind a sonic clone that emits disruptive pulses. Hover Wheels combined with Plasma Trail turns a racer into an area-denial specialist, carving burning paths across the battlefield. A Drone Forge linked with Guardian Dome allows support drones to construct and maintain shield emitters around objectives. Pairing Growth Matrix with Armor Lock transforms a player into a temporary walking fortress capable of anchoring a defensive line, while Compression Matrix plus Hunter Vision creates a highly mobile assassin that slips behind enemy formations.

Relic Evolution

Rather than replacing relics every few hours, players would restore them by finding matching fragments across raids. A relic might evolve through four restoration tiers:

Damaged – the newly recovered artifact with a simple ability.
Restored – improved performance and reduced cooldowns.
Masterworked – unlocks alternate behavior or a secondary passive.
Ascended – gains cosmetic transformation changes and unique synergy slots.

For example, a damaged Blink Drive only grants a short teleport. An ascended Blink Drive might chain two blinks, leave a holographic decoy, or briefly grant active camouflage after each teleport.

The Long-Term Goal

Instead of chasing ever-higher numbers, players spend the game assembling their own engineering philosophy. One player might become a stealth infiltrator with blink drives, active camouflage, and scouting drones. Another becomes a rolling siege engine with modular armor, growth matrices, deployable factories, and hover-tank modes. A third focuses on battlefield control with resonance pulses, gravity wells, and guardian shields.

By the endgame, two players may both be "Transformers," yet look, transform, and fight in completely different ways because they've reconstructed different branches of ancient Cybertronian technology from the relics scattered around the Ninth Forge. That kind of horizontal progression keeps older raids valuable indefinitely—every relic remains desirable because it unlocks a new capability or combination rather than merely offering a slightly bigger damage number.

Pasted text(26).txt
Document
how to evolve these systems to get there?

I think the key evolution is to stop thinking of this as a loot system and start thinking of it as an engineering system.

Most MMOs look like this:

Kill boss
↓
+5 Sword
↓
Higher DPS
↓
Next boss

A Transformer game should instead feel like you're rebuilding an extinct civilization.

Explore
↓
Recover ancient engineering
↓
Reverse engineer
↓
Prototype
↓
Install
↓
Discover synergies
↓
Unlock entirely new playstyles

That changes almost every progression system.

Phase 1 — Fixed Relics

This is where I'd start.

Each raid gives one signature relic.

Speakerman
    Resonance Core

Hardboil
    Inertial Gyroscope

Pyrachnid
    Thermal Weave

Gigadrill
    Excavation Servo

These are unique.

Simple.

Easy to balance.

Phase 2 — Relic Trees

Instead of leveling your character...

restore relics.

Example

Blink Drive

Recovered
     │
     ├── Faster Blink
     │
     ├── Longer Blink
     │
     ├── Double Blink
     │
     └── Blink leaves decoy

Now players specialize.

Phase 3 — Engineering

Now players discover relic combinations.

Not recipes.

Actual engineering.

Blink Drive

+

Resonance Core

↓

Phase Echo

Nobody handed them that.

They discovered it.

Another

Forge Core

+

Spider Legs

↓

Spider Factory

Your drones now climb walls.
Phase 4 — Rewrite the Chassis

Now relics begin changing geometry.

Instead of

+15 armor

they say

Replace lower chassis.

Gain:

Reverse knee joints

+20% sprint

Higher jumps

Less stability

Robot actually changes.

Phase 5 — Engineering Disciplines

This is where I think your game becomes unique.

Every relic belongs to a forgotten Cybertronian school.

Resonance

Forge

Hunter

Siege

Pioneer

Guardian

Phase

Temporal

Adaptive

Titan

Installing several from one school unlocks passive engineering bonuses.

Example

3 Resonance

↓

Sonic weapons charge faster.

5 Resonance

↓

Transformation emits EMP pulse.

Or mix schools

2 Hunter

3 Stealth

↓

Predator Package

Entirely new passive.

Phase 6 — Alternate Transformation Graphs

This is something I don't think any Transformer game has really explored.

Instead of

Robot

↓

Car

it becomes

Robot

↓

Car

↓

Hover

↓

Combat

↓

Siege

Each node is its own transform.

          Robot
          /   \
     Racer     Hover
         \      /
        Assault
            |
        Siege Mode

Every vehicle family gains its own graph.

Phase 7 — Body Architecture

Instead of cosmetic skins...

players unlock engineering packages.

Example

Legs

Choose

○ Human

○ Digitigrade

○ Reverse Joint

○ Spider

○ Tank Treads

○ Hover Pods

Each changes gameplay.

Arms

○ Heavy

○ Precision

○ Four Arm

○ Forge Arms

○ Mining Arms

○ Shield Arms

Back

○ Wings

○ Drone Rack

○ Missile Bay

○ Radar Mast

○ Reactor

○ Turbine

Eventually every player looks different.

Phase 8 — Component Damage

Now geometry matters.

Destroying

Radar

↓

Can't lock missiles.

Destroy

Thruster

↓

Can't hover.

Lose

Wheel

↓

Vehicle pulls right.

This makes repair bots meaningful.

Phase 9 — Manufacturing

This is where Forge relics become incredible.

Instead of summoning pets...

actually manufacture equipment.

Deploy Forge

↓

Harvest debris

↓

Manufacture

Turrets

Ammo

Repair kits

Drone allies

Bridge pieces

Shield generators

Players become engineers.

Phase 10 — Transformation Research

One of my favorite ideas.

Instead of new classes...

players research transformations.

Suppose someone defeats five hover relics.

Eventually they unlock

Hover Wheels

Any compatible wheeled chassis can now evolve into

Car

↓

Hover Car

without needing a separate model.

Later

Hover Car

↓

VTOL

Then

VTOL

↓

Space Flight

The transformation graph itself evolves.

Phase 11 — Relic Mutation

Very late game.

Relics begin affecting one another.

Example

Flamethrower

+

Cryo Core

normally impossible.

Eventually

Thermal Shock

Rapid heating/cooling causes enemies to fracture.

Or

Blink

+

Armor Lock

↓

Blink immediately enters Armor Lock.

Phase 12 — Ancient Reconstruction

The coolest endgame.

Instead of collecting loot...

players are rebuilding lost Cybertronian technology.

Example progression

Recover

Forge Core

↓

Repair

Forge Core

↓

Learn Blueprint

↓

Manufacture Prototype

↓

Install

↓

Mass Produce

↓

Faction gains access.


Eventually

entire player factions

reconstruct lost technologies.

Maybe one server finally restores


Quantum Blink


Another restores


Titan Armor


Another discovers


Soundwave Drone Swarms.


The world permanently evolves.

---

## My favorite evolution, though...

I'd replace traditional classes with **Engineering Licenses**.

Instead of selecting "Scout" or "Tank" at character creation, every player begins as a general Cybertronian and gradually earns certifications by mastering relic technologies.


Mobility License

- Hover systems
- Multi-mode transformations
- VTOL
- Blink drives


Forge License

- Drone factories
- Construction systems
- Repair platforms
- Mobile manufacturing


Hunter License

- Active camouflage
- Tracking optics
- Ambush abilities
- Pack coordination


Siege License

- Heavy cannons
- Deployable cover
- Artillery modes
- Armor Lock


Titan License

- Temporary size increase
- Crew-served weapons
- Walking fortress modes
- Squad support auras

You aren't *locked into* one discipline. Instead, your Transformer becomes a reflection of the engineering traditions you've recovered and studied. Two players could both start as similar vehicle types, yet hundreds of hours later one has evolved into a stealthy triple-form infiltration specialist with drone support, while another has become a towering mobile siege platform with deployable factories and resonance shields.

That progression feels much closer to the fantasy of **rediscovering and rebuilding an ancient technological civilization** than simply equipping stronger gear. It also fits your lore around the Ninth Forge: every relic recovered is another missing chapter in Cybertron's engineering history, and every new ability is not "magic" but a recovered piece of forgotten science.
```

### Beyond IQM

Adding some extra features to IQM is likely a good additional step for the game engine.

```
Based on the current tooling and your updated asset pipeline, I think this is the next major evolution.

New pipeline

Instead of the current "extract → simplify → compress textures", make it a proper asset compiler.

Downloads/
    *.zip
      │
      ▼
━━━━━━━━━━━━━━━━━━━━━━━━━━━
Stage 0
Discovery
━━━━━━━━━━━━━━━━━━━━━━━━━━━

scan archive
fingerprint
guess asset type
extract metadata

↓

━━━━━━━━━━━━━━━━━━━━━━━━━━━
Stage 1
Extraction
━━━━━━━━━━━━━━━━━━━━━━━━━━━

zip
rar
7z
tar
fbx
blend
gltf
obj
dae
abc

↓

━━━━━━━━━━━━━━━━━━━━━━━━━━━
Stage 2
Normalization
━━━━━━━━━━━━━━━━━━━━━━━━━━━

fix axes
meters
pivot
triangulate
merge duplicate verts
remove degenerate tris
generate tangents
generate normals

↓

━━━━━━━━━━━━━━━━━━━━━━━━━━━
Stage 3
Optimization
━━━━━━━━━━━━━━━━━━━━━━━━━━━

LOD generation
texture resize
BC compression
meshoptimizer
draco (optional)

↓

━━━━━━━━━━━━━━━━━━━━━━━━━━━
Stage 4
Conversion
━━━━━━━━━━━━━━━━━━━━━━━━━━━

OBJ
FBX
GLTF
DAE
↓

IQM

↓

━━━━━━━━━━━━━━━━━━━━━━━━━━━
Stage 5
Metadata synthesis
━━━━━━━━━━━━━━━━━━━━━━━━━━━

asset.json
tags.json
search index

↓

Game Engine
Extend to OBJ → IQM

Right now your tooling seems focused mostly on textures.

I'd extend it into a generic importer.

AssetImporter

supports:

OBJ
FBX
GLTF
GLB
DAE
ABC
BLEND (through Blender)

↓

IntermediateScene

↓

IQM exporter

The important part is never convert directly.

Everything should first become an intermediate representation.

Example:

Scene

Meshes

Materials

Textures

Bones

Animations

Cameras

Lights

Metadata

Then

Scene
      ↓
OBJ importer

Scene
      ↓
IQM exporter

Scene
      ↓
Engine

This lets every format share the same optimization code.

IQM advantages

IQM is a surprisingly good runtime format.

It already supports

skeletal animation
multiple animations
joints
vertex weights
tangents
normals
UVs
animation clips

For Transformers it's almost ideal.

Vehicle meshes

↓

Robot meshes

↓

Animation

↓

Engine

OBJ limitations

Remember OBJ contains almost nothing.

Usually

positions

normals

uv

material name

No

skeleton
animations
constraints
hierarchy

So your importer should detect

if OBJ

    static mesh

else if FBX

    import animation

else if GLTF

    import skeleton

else if IQM

    pass through
IQM conversion

Rather than writing your own exporter immediately, I'd wrap existing converters first.

OBJ

↓

Assimp

↓

Intermediate Scene

↓

iqm exporter

or

OBJ

↓

Blender (headless)

↓

IQM exporter addon

↓

IQM

Later you can replace the exporter.

Metadata extraction

Your filenames are actually a gold mine.

Example

flyer--Sea Harrier FA2 with AIM-9L & AIM-120B.zip

should become

{
    "role": "flyer",
    "vehicle": "Sea Harrier FA2",

    "weapons": [
        "AIM-9L",
        "AIM-120B"
    ],

    "mobility": "air",

    "size": "medium",

    "faction": null,

    "patrol_candidate": true
}

Another

raidboss--primitive-mantis-mech.zip

↓

{
    "category":"raidboss",
    "species":"mantis",
    "size":"huge",
    "role":"boss",
    "legs":6
}

This metadata can drive spawning without hand-tagging thousands of assets.

Folder output

I'd stop dumping everything beside the extracted files.

Instead

processed/

    asset-id/

        source/

        meshes/

            high.iqm

            lod0.iqm

            lod1.iqm

            lod2.iqm

        textures/

            albedo.ktx2
            normal.ktx2
            orm.ktx2

        metadata/

            asset.json

            physics.json

            ai.json

        previews/

            front.png
            iso.png
            wireframe.png

        logs/

            optimize.txt
            conversion.txt
Long-term engine change

Eventually I'd remove the assumption that the engine only loads IQM.

Instead:

AssetManager

↓

Importer Registry

OBJ

FBX

GLTF

IQM

↓

Scene Graph

↓

GPU Mesh

↓

Renderer

That gives you editor flexibility while still caching optimized IQM assets for shipping builds. A practical workflow is to import any supported source format into the intermediate scene representation, generate optimized IQM files plus compressed textures as a build artifact, and have the engine load those cached assets by default while retaining the original sources for re-import when your processing pipeline improves.

is iqm or obj better compression and load time?

For a game engine, IQM wins by a large margin in almost every metric except editability.

Metric	OBJ	IQM
Disk size	❌ Large ASCII	✅ Compact binary
Compression (zip/zstd)	Good	Very good
Parse speed	❌ Slow	✅ Very fast
Memory allocations	❌ Many	✅ Few
GPU upload	❌ Requires conversion	✅ Nearly direct
Animation	❌ None	✅ Built in
Skeletal mesh	❌ No	✅ Yes
Random access	❌ Poor	✅ Excellent
File size

OBJ is plain text.

Example:

v 0.1234 0.3456 0.8888
v 0.1244 0.3466 0.8898
vn ...
vt ...
f 1/1/1 2/2/2 3/3/3

Every number is written as characters.

A million vertices means millions of ASCII floats.

IQM instead stores

float
float
float

float
float

directly as binary.

Typical results:

Mesh	OBJ	IQM
50k triangles	4 MB	1.2 MB
250k triangles	22 MB	6 MB
1M triangles	90 MB	24 MB
Compression

Interestingly, OBJ compresses fairly well because text is repetitive.

OBJ
90 MB

↓

zstd

↓

14 MB

IQM

24 MB

↓

zstd

↓

10 MB

So compressed archives are often surprisingly close.

Runtime, however, is a different story.

Load time

OBJ must parse text.

"0.345634"

↓

ASCII

↓

float

↓

store

That conversion dominates load time.

IQM simply does

read()

↓

memcpy()

↓

done

Typical comparison:

Model	OBJ	IQM
100k verts	25–70 ms	2–5 ms
1M verts	250–700 ms	15–40 ms

On slower CPUs (Steam Deck, ARM laptops, mobile), the gap can be even larger.

Memory

OBJ usually requires several temporary structures:

ASCII

↓

Tokenizer

↓

Strings

↓

Indices

↓

Vertices

↓

GPU buffers

Peak memory can be 2–4× the final mesh size.

IQM is much closer to:

File

↓

Mapped

↓

GPU

Minimal temporary allocations.

CPU cache friendliness

IQM stores arrays like:

positions[]

normals[]

uvs[]

weights[]

triangles[]

OBJ stores a stream of text that must be parsed and scattered into memory.

Binary layouts are dramatically more cache-friendly.

Animation

OBJ:

Static mesh only

IQM:

Mesh

Skeleton

Animations

Weights

Poses

Frames

One IQM file can contain dozens of animation clips, making it ideal for characters or transforming entities.

Engine startup

Suppose your game loads 500 assets.

OBJ
read file

↓

parse text

↓

convert floats

↓

build arrays

↓

deduplicate

↓

generate tangents

↓

upload GPU
IQM
read file

↓

validate header

↓

map arrays

↓

upload GPU

That's why engines typically convert editable assets into binary runtime formats during the build process.

If you're designing your own engine

I'd go one step further and create a custom runtime format inspired by IQM.

source/

    robot.obj
    robot.fbx
    robot.gltf

↓

asset compiler

↓

robot.uasset

Where robot.uasset contains:

Header

Meshes

LOD table

Vertex buffer

Index buffer

Material table

Bounding boxes

Collision

Animation clips

Compressed textures

Optional meshlets

Metadata

CRC

Version

You can even memory-map the file and point directly into its sections without additional parsing.

Overall recommendation
OBJ: Use only as an interchange format between modeling tools or for importing third-party assets.
IQM: A strong choice as a runtime format if you want a proven, lightweight solution with skeletal animation support.
Long-term: For your engine, consider compiling everything into a custom binary asset container that extends IQM concepts. You could embed IQM-compatible mesh and animation data alongside engine-specific features such as LODs, meshlets, compressed textures (e.g. KTX2), collision data, metadata, and streaming information. That gives you IQM's fast loading while eliminating the need for multiple runtime asset files.
```

here is a further development of my thoughts:
```
and what about generating thruster data and traction data and center of mass data from the models themselves?

I think that's a much stronger direction than trying to infer everything from the mesh alone.

What you're really doing is shifting from geometry inference to generative anatomy (or more generally, a parametric internal structure). The mesh constrains the solution, but the internals come from archetypes.

For example, instead of asking:

"Where is the center of mass?"

you ask:

"What plausible internals fit inside this shell?"

Think in layers
Visible Mesh

↓

Body Classification

↓

Internal Layout Template

↓

Fit To Shell

↓

Physics

The mesh is no longer the source of truth—it's a container.

Archetypes

You might have something like:

Humanoid

    skinny

    athletic

    muscular

    heavy

    obese

    armored

    giant

    robot

    insectoid

    digitigrade

    quadruped

Each archetype contains a parametric internal layout.

For example:

Heavy Armored

Torso
██████████

Core
████

Battery
██

Shoulders
████

Leg actuators
█████

versus

Slim Scout

Torso
██

Battery
█

Sensors
███

Legs
██████
The mesh constrains the template

Suppose you import

optimus.obj

You detect

humanoid
broad shoulders
truck cab chest
thick legs

The system picks

heavy armored

Then scales and morphs that internal template until it fits inside the shell.

That gives you

center of mass
inertia tensor
battery location
fuel location (if applicable)
armor thickness
weak spots
cargo volume









that's right the compiler might need layers and abstractions for terrain, buildings, doodadse, missiles/bullets, and transformers because I have reference models and I doubt telling claude to model noise to PSNR match these reference models is going to get me all the way there on the rest of the game I have been hyperfocused on the real challenges in the bot and vehicle area



but that doesn't make those challenges lesser obstacles. I was thinking of a traits system but the tricky part is there is not enough differentiation in the models alone some sort of wholistic believability score for each of the types should be generating subsets of numbers of component traits to look for in the model with a mean and stddev output and then that data can be matched with the chosen class by the player and their bot form to further constrain



for debugging I want to compress the input material a bit because it sits at almost 100GB, then generate better models based off of the compressed input material and two or three good high detailed "innards" models, and then pipe those to the modern formats I will build into the above analysis framework where there is a what feels and what looks slider and the two models are different but paired they get the look and the feel down with minimal differences



on your last critique about not touching speed and armor, if you don't make that believable by default then the immersion falls apart so there has to be a way to carry over range adjusters of the traits, like maybe when running heavy bots that want to go fast have to research their rocket backs or their double jump boosters, or lighter vehicles have to research shielding technology, deployable scramblers, or better evasive maneuvering and crouching fighting stances or peeking out of cover or missiles that bend corners and have tracking to feel more realistic. I also have various world types and I want to encourage the players to take on new forms when they feel constrained instead of building hand-holders for each vehicle type on each map. This also makes the game balance different based on the environment which let's players not feel bad about choosing their favorite over the meta in the PvE group sessions.

I think you're converging on something that's much more interesting than "an importer." You're describing a knowledge compiler that translates from visual geometry to simulation geometry. That's a hard problem, but I think there's one place where I'd change your architecture significantly.

I think you're still trying to solve everything in one pass

Right now I hear something like:

Mesh

↓

Detect traits

↓

Choose archetype

↓

Generate internals

↓

Generate gameplay

I think this is too eager. Every decision biases the next one.

Instead I'd build it almost like a compiler.

Mesh

↓

Measurements

↓

Features

↓

Evidence

↓

Hypotheses

↓

Constraint Solving

↓

Internal Model

↓

Derived Physics

↓

Gameplay Suggestions

The important part is that nothing commits early.

I love your "believability score", but I'd rename it

I wouldn't call it a score.

I'd call it an evidence vector.

Instead of

Tank = 0.82
Car = 0.12
Jet = 0.06

have something like

Features

Long = 0.91

Wide = 0.77

Symmetric = 0.99

Ground Contact = 0.85

Wing Area = 0.05

Branch Count = 0.01

Wheel Evidence = 0.73

Cockpit Evidence = 0.62

Engine Evidence = 0.54

Now every archetype asks different questions.

A tank cares about

Ground Contact

Track Evidence

Hull Volume

Turret Evidence

A jet cares about

Wing Area

Tail

Nozzles

Aspect Ratio

You're comparing evidence against expectations instead of assigning a label first.

I think the player's choice is actually another constraint

This part really clicked for me:

matched with the chosen class by the player and their bot form

Don't think of that as overriding the compiler.

Treat it as another source of evidence.

For example:

Geometry

↓

Evidence

says

Tank : 0.42

Truck : 0.39

Mech : 0.19

Player chooses

Tank

Now the solver says

"Okay, I'll explain this geometry as a tank."

It doesn't force impossible things.

It searches for the most believable tank explanation.

Compressing the 100 GB reference library

I actually think this is one of the most important parts of your pipeline.

But I wouldn't compress it for storage.

I'd compress it into knowledge.

Instead of

100 GB

↓

20 GB

I'd want

100 GB

↓

Feature Database

↓

Statistical Priors

Imagine processing every reference model and recording things like:

Tank

Length Ratio

mean 1.73

σ 0.18

Ground Clearance

mean 0.42

σ 0.07

Turret Radius

mean 0.33

σ 0.09

Now you don't need thousands of tanks anymore.

You have distributions.

This is much stronger than nearest-neighbor

You're currently thinking

compare against references

I'd think

learn distributions

Instead of

Tank #52

you get

Tank Family

which is much more general.

I really like the "looks" versus "feels" slider

I think that's actually two entirely different models.

Not

Mesh

↓

Physics

but

Visual Representation

↓

Simulation Representation

They're siblings.

Reference

↓

Visual Model

↓

Simulation Model

↓

Paired Asset

The simulation model can simplify

armor thickness
mass
collision
internal components

while preserving the same perceived object.

I think gameplay should influence internals, not the other way around

This is where I'd push back slightly.

You wrote:

speed and armor need to feel believable

I completely agree.

But I'd implement it differently.

Instead of

Mesh

↓

Armor

↓

Speed

I'd do

Desired Gameplay

↓

Internal Components

↓

Derived Physics

↓

Visual Validation

Suppose the designer wants

Heavy

Fast

That's not impossible.

The solver asks

"What makes that believable?"

Possible answers

larger engine
fusion reactor
jump jets
overboost
thinner armor
exotic materials

The game then explains the capability.

That's much more satisfying than arbitrary stat bonuses.

I think your research tree idea is the strongest design point

This part stood out:

heavy bots that want to go fast research rocket backs

That's not just progression.

That's causal progression.

Instead of

+20% Speed

the player gains

Booster Pack

Now

Heavy

+

Booster

↓

Fast

The player understands why.

Likewise

Scout

↓

Shield Generator

↓

Survivability

or

Tank

↓

Adaptive Tracks

↓

Better Snow Mobility

Everything has a physical explanation.

Environment-dependent balance

I honestly think this is one of the most promising ideas you've mentioned over the past week.

Instead of balancing

Tank

Scout

Jet

globally,

balance them against environments.

Dense Forest

Urban

Open Desert

Mountain

Ice

Low Gravity

Cybertron

Underground

Now the optimal answer changes.

Players can succeed by adapting rather than chasing a single universally optimal build.

That's a much healthier design space for cooperative PvE because diversity becomes valuable.

One thing I would still challenge

I think you're still assuming there is one "correct" internal layout.

I don't think there is.

Imagine this mesh.

█████████████

Five different believable internals may fit.

Instead of generating

Internal Layout

generate

Candidate A

92%

Candidate B

87%

Candidate C

81%

Each satisfies

geometry
chosen archetype
gameplay
player selections

Then either pick the highest-scoring one or let designers inspect alternatives.

That's much closer to how constraint solvers work than how classifiers work.

The architecture I would aim for

If I were sketching the system today, it would look something like this:

                Authoring Assets
                      │
          (OBJ, IQM, GLTF, CTEX, etc.)
                      │
                      ▼
              Geometry Analyzer
                      │
     (measurements, topology, symmetry,
      appendages, cavities, materials)
                      │
                      ▼
              Evidence Database
                      │
          +-------------------------+
          |                         |
          ▼                         ▼
  Statistical Priors        Player Constraints
 (reference library)      (chosen form, faction,
                            desired role, tech)
          \                         /
           \                       /
            ▼                     ▼
             Constraint Solver
                    │
      Generates candidate internals
                    │
        +-----------+-----------+
        |                       |
        ▼                       ▼
 Visual Representation   Simulation Representation
        |                       |
        +-----------+-----------+
                    │
                    ▼
        Physics, AI, Damage, Gameplay

The reason I like this architecture is that every stage has a single responsibility. The analyzer measures facts, the evidence database records observations, the priors describe what is typical, the player adds intent, the solver reconciles those constraints, and only then do you derive gameplay. That separation makes the system easier to debug, easier to improve, and much more resilient as you add entirely new categories like terrain, buildings, vegetation, missiles, or creatures without rewriting the whole pipeline.
```
