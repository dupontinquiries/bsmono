/* test_transport.c
  Runs as either a UDP server or client (argv[1] = "server"|"client").
  The server authoritatively "destroys" a chunk and sends a RELIABLE_ORDERED
  EX_MSG_S_CHUNK_STATE + EX_MSG_S_EVENT_FIRED to the client over a real socket.
  The client waits for both and verifies it received the authoritative state.

  Demonstrates the reliability layer works over actual UDP (not loopback
  memcpy): run two processes and watch the client converge on server state.
*/

#include "net_transport.h"
#include "netcode.h"
#include <stdio.h>
#include <string.h>
#include <time.h>

#ifdef _WIN32
  #include <windows.h>
  static void sleep_ms(int ms){ Sleep(ms); }
  static double now_ms(void){ return (double)GetTickCount(); }
#else
  #include <unistd.h>
  static void sleep_ms(int ms){ usleep(ms*1000); }
  static double now_ms(void){ struct timespec ts; clock_gettime(CLOCK_MONOTONIC,&ts);
                              return ts.tv_sec*1000.0 + ts.tv_nsec/1e6; }
#endif

#define PORT 45999

static int run_server(void)
{
  ex_net_peer_t peer;
  if (ex_net_server_open(&peer, PORT) != 0) { printf("[server] bind failed\n"); return 1; }
  printf("[server] listening on udp %d\n", PORT);

  // wait for the client's first packet so we learn its address
  ex_net_packet_t in;
  int got_hello = 0;
  for (int i = 0; i < 500 && !got_hello; i++) {
    if (ex_net_recv(&peer, &in)) got_hello = 1;
    ex_net_update(&peer, now_ms());
    sleep_ms(10);
  }
  if (!got_hello) { printf("[server] no client hello, aborting\n"); return 1; }
  printf("[server] client connected; applying authoritative chunk destruction\n");

  // authoritative state: send chunk destroyed + event, both reliable-ordered
  ex_net_packet_t p1, p2;
  ex_net_chunk_state_t cs;
  strncpy(cs.chunk_name, "arena_wall_01", sizeof(cs.chunk_name));
  cs.new_state = 2; // DESTROYED
  cs.hp = 0.0f;
  ex_net_pack_chunk_state(&p1, &cs);

  ex_net_event_fired_t ev = { .int_arg = 0, .float_arg = 0.0f };
  strncpy(ev.event_name, "wall_breached", sizeof(ev.event_name));
  ex_net_pack_event_fired(&p2, &ev);

  ex_net_send(&peer, &p1);
  ex_net_send(&peer, &p2);
  printf("[server] sent CHUNK_STATE + EVENT_FIRED (reliable-ordered)\n");

  // keep resending unacked reliables until acked (or timeout)
  for (int i = 0; i < 300; i++) {
    ex_net_recv(&peer, &in);              // drain acks
    ex_net_update(&peer, now_ms());       // resend if needed
    int outstanding = 0;
    for (int k = 0; k < EX_NET_MAX_RESEND; k++)
      if (peer.resend[k].in_use) outstanding++;
    if (outstanding == 0) { printf("[server] all reliable packets acked by client\n"); break; }
    sleep_ms(10);
  }
  ex_net_peer_close(&peer);
  return 0;
}

static int run_client(void)
{
  ex_net_peer_t peer;
  if (ex_net_client_open(&peer, "127.0.0.1", PORT) != 0) { printf("[client] open failed\n"); return 1; }
  printf("[client] connecting to 127.0.0.1:%d\n", PORT);

  // say hello so the server learns our address (a JOIN in the real protocol)
  ex_net_packet_t hello; memset(&hello, 0, sizeof(hello));
  hello.type = EX_MSG_C_JOIN; hello.protocol_version = EX_NET_PROTOCOL_VERSION; hello.payload_len = 0;
  ex_net_send(&peer, &hello);

  int got_chunk = 0, got_event = 0;
  ex_net_packet_t in;
  for (int i = 0; i < 800 && !(got_chunk && got_event); i++) {
    // resend hello occasionally until server responds
    if (i % 20 == 0 && !got_chunk) ex_net_send(&peer, &hello);

    while (ex_net_recv(&peer, &in)) {
      if (in.type == EX_MSG_S_CHUNK_STATE) {
        ex_net_chunk_state_t cs;
        if (ex_net_unpack_chunk_state(&in, &cs)) {
          printf("[client] CHUNK_STATE '%s' state=%d hp=%.0f\n", cs.chunk_name, cs.new_state, cs.hp);
          got_chunk = 1;
        }
      } else if (in.type == EX_MSG_S_EVENT_FIRED) {
        ex_net_event_fired_t ev;
        if (ex_net_unpack_event_fired(&in, &ev)) {
          printf("[client] EVENT_FIRED '%s'\n", ev.event_name);
          got_event = 1;
        }
      }
    }
    ex_net_update(&peer, now_ms());
    sleep_ms(10);
  }

  ex_net_peer_close(&peer);
  if (got_chunk && got_event) {
    printf("[client] SUCCESS: received authoritative chunk destruction + event over real UDP\n");
    return 0;
  }
  printf("[client] FAILED: chunk=%d event=%d\n", got_chunk, got_event);
  return 1;
}

int main(int argc, char **argv)
{
  ex_net_transport_init();
  int rc;
  if (argc > 1 && strcmp(argv[1], "server") == 0) rc = run_server();
  else if (argc > 1 && strcmp(argv[1], "client") == 0) rc = run_client();
  else { printf("usage: %s server|client\n", argv[0]); rc = 2; }
  ex_net_transport_shutdown();
  return rc;
}
