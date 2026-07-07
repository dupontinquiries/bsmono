/* netcode
  Minimal server-authoritative wire protocol tying together the
  systems built so far:
    - coll_chunk_t state changes (damage, destroy, seal/open)
    - event bus fires (airship_destroyed, objective_held, etc)
    - character transform mode flips

  Design stance (matches the earlier design doc / coll_chunk notes):
  the SERVER is the only place damage is applied, chunk state is
  flipped, or events are fired. Clients send INPUT/requests; the
  server sends STATE. This file defines the wire format + a
  transport-agnostic send/receive pipeline. It intentionally does
  NOT include a socket layer -- exengine has none today, and sockets
  are OS-specific plumbing that should be swapped in later (enet,
  raw UDP+reliability, or a platform abstraction). What's here is
  the part that's actually hard to get right: message shape,
  ordering/reliability requirements per message type, and
  reconciliation with the local simulation.

  A loopback transport (net_loopback.c) is provided for testing the
  protocol end-to-end in a single process before real sockets exist.
*/

#ifndef EX_NETCODE_H
#define EX_NETCODE_H

#include <stdint.h>
#include <stddef.h>

#define EX_NET_MAX_PACKET_BYTES 512
#define EX_NET_PROTOCOL_VERSION 1

typedef enum {
  // client -> server (input/requests, never trusted as truth)
  EX_MSG_C_MOVE_INPUT       = 1,  // reliable: no. sent every tick, unreliable ok
  EX_MSG_C_TRANSFORM_REQUEST= 2,  // reliable
  EX_MSG_C_DAMAGE_REQUEST   = 3,  // reliable -- "I think I hit chunk X for Y" (server verifies)
  EX_MSG_C_JOIN             = 4,  // reliable

  // server -> client (authoritative state, must be reliable+ordered
  // for anything permanent; snapshot/input state can be unreliable)
  EX_MSG_S_CHUNK_STATE      = 100, // reliable+ordered: permanent map changes
  EX_MSG_S_EVENT_FIRED      = 101, // reliable+ordered
  EX_MSG_S_TRANSFORM_STATE  = 102, // reliable: authoritative mode+state for one character
  EX_MSG_S_SNAPSHOT         = 103, // unreliable, high frequency: positions/velocities
  EX_MSG_S_JOIN_ACCEPT      = 104, // reliable
} ex_net_msg_type;

// reliability classes -- transport layer (future sockets) must honor these
typedef enum {
  EX_NET_UNRELIABLE,       // ok to drop, superseded by next snapshot
  EX_NET_RELIABLE,         // must arrive, order doesn't matter
  EX_NET_RELIABLE_ORDERED  // must arrive, in order (permanent map/game state)
} ex_net_reliability;

typedef struct {
  uint8_t  type;
  uint8_t  protocol_version;
  uint16_t payload_len;
  uint8_t  payload[EX_NET_MAX_PACKET_BYTES];
} ex_net_packet_t;

// ---- payload structs (packed manually via serialize/deserialize below) ----

typedef struct {
  char     chunk_name[48];
  uint8_t  new_state;    // ex_chunk_state
  float    hp;
} ex_net_chunk_state_t;

typedef struct {
  char     event_name[48];
  int32_t  int_arg;
  float    float_arg;
} ex_net_event_fired_t;

typedef struct {
  int32_t  owner_id;
  uint8_t  mode;   // ex_xform_mode
  uint8_t  state;  // ex_xform_state
} ex_net_transform_state_t;

typedef struct {
  int32_t owner_id;
  float   pos[3];
  float   vel[3];
  uint8_t mode;
} ex_net_snapshot_entry_t;

/**
 * [ex_net_reliability_for classify a message type's required reliability]
 * The (future) real transport layer should route packets accordingly
 * (e.g. an ENet reliable-ordered channel for RELIABLE_ORDERED types).
 */
ex_net_reliability ex_net_reliability_for(ex_net_msg_type type);

/**
 * [ex_net_pack_chunk_state serialize a chunk-state message into a packet]
 */
void ex_net_pack_chunk_state(ex_net_packet_t *pkt, const ex_net_chunk_state_t *data);
int  ex_net_unpack_chunk_state(const ex_net_packet_t *pkt, ex_net_chunk_state_t *out);

void ex_net_pack_event_fired(ex_net_packet_t *pkt, const ex_net_event_fired_t *data);
int  ex_net_unpack_event_fired(const ex_net_packet_t *pkt, ex_net_event_fired_t *out);

void ex_net_pack_transform_state(ex_net_packet_t *pkt, const ex_net_transform_state_t *data);
int  ex_net_unpack_transform_state(const ex_net_packet_t *pkt, ex_net_transform_state_t *out);

#endif // EX_NETCODE_H
