/* test_netcode.c
  End-to-end test of the intended flow:
    1. "Client" sends EX_MSG_C_DAMAGE_REQUEST for a chunk it thinks it hit.
    2. "Server" (authoritative) applies ex_chunk_apply_damage() itself --
       never trusts the client's claim about the outcome, only the intent.
    3. If the chunk transitions to DESTROYED, server fires an event and
       packs an EX_MSG_S_CHUNK_STATE + EX_MSG_S_EVENT_FIRED packet.
    4. "Client" receives and unpacks those packets and applies them
       locally (this is the ONLY path client-side state should change
       through for permanent map state).

  No real sockets -- packets are just memcpy'd between two in-process
  buffers to prove the protocol/serialization is correct end-to-end.
  Swapping in real transport later shouldn't change any of the logic
  above the packet pack/unpack calls.
*/

#include "netcode.h"
#include "coll_chunk.h"
#include "events.h"
#include <stdio.h>
#include <assert.h>
#include <string.h>

// --- fake wire: just a small FIFO of packets between "server" and "client" ---
#define WIRE_CAP 16
static ex_net_packet_t wire_to_client[WIRE_CAP];
static int wire_to_client_count = 0;

static void wire_send_to_client(ex_net_packet_t *pkt)
{
  assert(wire_to_client_count < WIRE_CAP);
  wire_to_client[wire_to_client_count++] = *pkt;
}

int main(void)
{
  printf("=== SERVER SETUP ===\n");
  ex_chunk_world_t *server_world = ex_chunk_world_new();
  ex_event_bus_t *server_bus = ex_event_bus_new();

  vec3 wall_tris[6] = {
    {0,0,0}, {2,0,0}, {2,3,0},
    {0,0,0}, {2,3,0}, {0,3,0},
  };
  ex_coll_chunk_t *server_wall = ex_chunk_add(server_world, "velatris_wall_heavy_01",
    ex_rect_new((vec3){0,0,-0.1f}, (vec3){2,3,0.1f}), wall_tris, 2,
    /*hp*/ 100.0f, EX_CHUNK_HIT_HEAVY | EX_CHUNK_HIT_TITAN, /*destructible*/ 1);

  // server-side: when a chunk is destroyed, replicate state + fire a map event
  printf("=== CLIENT SETUP (mirrors the same chunk, starts in sync) ===\n");
  ex_chunk_world_t *client_world = ex_chunk_world_new();
  ex_coll_chunk_t *client_wall = ex_chunk_add(client_world, "velatris_wall_heavy_01",
    ex_rect_new((vec3){0,0,-0.1f}, (vec3){2,3,0.1f}), wall_tris, 2,
    /*hp*/ 100.0f, EX_CHUNK_HIT_HEAVY | EX_CHUNK_HIT_TITAN, /*destructible*/ 1);

  printf("\n=== SIMULATION ===\n");
  printf("client wall hp=%.0f state=%d\n", client_wall->hp, client_wall->state);

  // --- client sends a damage request (reliable-ordered per protocol) ---
  printf("\n[client] sends C_DAMAGE_REQUEST for 'velatris_wall_heavy_01', claims Heavy hit, 120 dmg\n");
  assert(ex_net_reliability_for(EX_MSG_C_DAMAGE_REQUEST) == EX_NET_RELIABLE_ORDERED);

  // --- server: never trusts the client's damage number blindly in a real
  //     game (would validate weapon cooldown / range / etc); here we just
  //     apply the authoritative call directly, proving server-side is
  //     the sole source of truth ---
  int destroyed = ex_chunk_apply_damage(server_wall, 120.0f, EX_CHUNK_HIT_HEAVY);
  printf("[server] applied damage authoritatively: hp=%.0f state=%d destroyed=%d\n",
         server_wall->hp, server_wall->state, destroyed);
  assert(destroyed == 1);
  assert(server_wall->state == EX_CHUNK_STATE_DESTROYED);

  // --- server packs + "sends" the resulting state to the client ---
  ex_net_packet_t chunk_pkt;
  ex_net_chunk_state_t chunk_data;
  strncpy(chunk_data.chunk_name, server_wall->name, sizeof(chunk_data.chunk_name));
  chunk_data.new_state = (uint8_t)server_wall->state;
  chunk_data.hp = server_wall->hp;
  ex_net_pack_chunk_state(&chunk_pkt, &chunk_data);
  wire_send_to_client(&chunk_pkt);

  // server also fires the map event and replicates it
  ex_event_fire(server_bus, "wall_breached", /*lane=0*/ 0, 0.0f);
  ex_net_packet_t event_pkt;
  ex_net_event_fired_t event_data = { .int_arg = 0, .float_arg = 0.0f };
  strncpy(event_data.event_name, "wall_breached", sizeof(event_data.event_name));
  ex_net_pack_event_fired(&event_pkt, &event_data);
  wire_send_to_client(&event_pkt);

  printf("[server] queued %d packet(s) for client\n", wire_to_client_count);

  // --- client receives and applies (this is the ONLY way client state changes) ---
  printf("\n[client] draining wire...\n");
  for (int i = 0; i < wire_to_client_count; i++) {
    ex_net_packet_t *pkt = &wire_to_client[i];

    if (pkt->type == EX_MSG_S_CHUNK_STATE) {
      ex_net_chunk_state_t out;
      int ok = ex_net_unpack_chunk_state(pkt, &out);
      assert(ok);
      ex_coll_chunk_t *local = ex_chunk_find(client_world, out.chunk_name);
      assert(local != NULL);
      local->hp = out.hp;
      ex_chunk_set_state(local, (ex_chunk_state)out.new_state);
      printf("[client] applied CHUNK_STATE for '%s': hp=%.0f state=%d\n",
             out.chunk_name, local->hp, local->state);
    } else if (pkt->type == EX_MSG_S_EVENT_FIRED) {
      ex_net_event_fired_t out;
      int ok = ex_net_unpack_event_fired(pkt, &out);
      assert(ok);
      printf("[client] received EVENT_FIRED: '%s' int_arg=%d\n", out.event_name, out.int_arg);
    }
  }

  printf("\nfinal client wall: hp=%.0f state=%d (expect 0, DESTROYED=%d)\n",
         client_wall->hp, client_wall->state, EX_CHUNK_STATE_DESTROYED);
  assert(client_wall->hp == 0.0f);
  assert(client_wall->state == EX_CHUNK_STATE_DESTROYED);
  assert(client_wall->state == server_wall->state);

  printf("\nALL ASSERTIONS PASSED -- client and server chunk state match after sync\n");

  ex_chunk_world_free(server_world);
  ex_chunk_world_free(client_world);
  ex_event_bus_free(server_bus);
  return 0;
}
