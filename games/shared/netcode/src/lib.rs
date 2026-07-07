//! netcode — the identity + discovery layer the fleet's games share. A player has one **Profile**
//! (id, name, presence, friends) published to a DHT so any game (and the bsgames centre) can find
//! who's online and what they're playing. Two backends behind one trait: **OpenDHT** (the global
//! primary) and a pure **micro-DHT** (a Kademlia-lite fallback for LAN/offline). Same graceful
//! floor the rest of the fleet uses — the network is a step-up, never a requirement.
pub mod microdht;
pub mod profile;
pub mod store;
