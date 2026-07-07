/* net_transport
  A real UDP socket transport for the netcode protocol (netcode.h).

  This is the piece netcode.h deliberately left out: the OS sockets +
  a minimal reliability layer. It's intentionally small and dependency
  -free (BSD sockets only, no ENet) so it builds anywhere exengine does,
  including aarch64 (Asahi) and Mesa desktop.

  Reliability model (matches ex_net_reliability_for in netcode.h):
    - EX_NET_UNRELIABLE       : fire-and-forget datagram (snapshots/input)
    - EX_NET_RELIABLE /
      EX_NET_RELIABLE_ORDERED : sender keeps the packet in a resend queue,
                                stamped with a sequence number, until the
                                peer acks it. Ordered delivery is enforced
                                on the receive side by a next-expected-seq
                                gate (out-of-order reliable packets are
                                buffered until their predecessor arrives).

  This is not a full congestion-control stack -- it's the ~minimum that
  makes permanent game state (chunk destruction, events) safe to send
  over lossy UDP, which is exactly what the design needs.
*/

#ifndef EX_NET_TRANSPORT_H
#define EX_NET_TRANSPORT_H

#include "netcode.h"
#include <stdint.h>
#include <stddef.h>

#ifdef _WIN32
  #include <winsock2.h>
  typedef SOCKET ex_socket_t;
#else
  typedef int ex_socket_t;
#endif

#define EX_NET_MAX_RESEND      64    // in-flight reliable packets per peer
#define EX_NET_RESEND_MS       120   // resend interval for unacked reliables
#define EX_NET_RECV_BUFFER     32    // out-of-order reliable hold buffer

// wire header prepended to every ex_net_packet_t on the socket
typedef struct {
  uint16_t seq;        // sequence number (reliable only; 0 for unreliable)
  uint16_t ack;        // highest contiguous seq the sender has received
  uint8_t  flags;      // bit0 = reliable, bit1 = is-ack-only
} ex_net_wire_header_t;

#define EX_NET_FLAG_RELIABLE 0x01
#define EX_NET_FLAG_ACKONLY  0x02

typedef struct {
  ex_net_packet_t packet;
  uint16_t seq;
  double   last_sent_ms;
  int      acked;
  int      in_use;
} ex_net_pending_t;

typedef struct {
  ex_socket_t sock;
  int         is_server;

  // peer address (server: last client; client: the server)
  unsigned char peer_addr[128];
  int           peer_addr_len;
  int           has_peer;

  // send-side reliability
  uint16_t next_seq;
  ex_net_pending_t resend[EX_NET_MAX_RESEND];

  // recv-side ordering
  uint16_t expected_seq;
  ex_net_packet_t hold[EX_NET_RECV_BUFFER];
  uint16_t        hold_seq[EX_NET_RECV_BUFFER];
  int             hold_used[EX_NET_RECV_BUFFER];

  uint16_t remote_ack; // last ack we received from peer
} ex_net_peer_t;

/**
 * [ex_net_transport_init one-time global init (Winsock on Windows, no-op elsewhere)]
 */
int ex_net_transport_init(void);
void ex_net_transport_shutdown(void);

/**
 * [ex_net_server_open bind a UDP server socket on the given port]
 */
int ex_net_server_open(ex_net_peer_t *peer, uint16_t port);

/**
 * [ex_net_client_open create a client socket and set the server as its peer]
 */
int ex_net_client_open(ex_net_peer_t *peer, const char *host, uint16_t port);

/**
 * [ex_net_send queue/send a packet with the reliability implied by its type]
 * Reliable packets are tracked for resend until acked.
 */
int ex_net_send(ex_net_peer_t *peer, const ex_net_packet_t *pkt);

/**
 * [ex_net_recv non-blocking: returns 1 and fills out_pkt if a packet is ready
 *  (in-order for reliable), 0 if nothing available this call]
 * Also processes incoming acks and records the peer address (server side).
 */
int ex_net_recv(ex_net_peer_t *peer, ex_net_packet_t *out_pkt);

/**
 * [ex_net_update resend un-acked reliable packets whose timer elapsed;
 *  call once per tick with the current time in ms]
 */
void ex_net_update(ex_net_peer_t *peer, double now_ms);

void ex_net_peer_close(ex_net_peer_t *peer);

#endif // EX_NET_TRANSPORT_H
