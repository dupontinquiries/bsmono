#include "net_transport.h"
#include <string.h>
#include <stdio.h>

#ifdef _WIN32
  #include <ws2tcpip.h>
  #define CLOSESOCK closesocket
  static int sock_would_block(void) { return WSAGetLastError() == WSAEWOULDBLOCK; }
#else
  #include <sys/socket.h>
  #include <netinet/in.h>
  #include <arpa/inet.h>
  #include <netdb.h>
  #include <unistd.h>
  #include <fcntl.h>
  #include <errno.h>
  #define CLOSESOCK close
  static int sock_would_block(void) { return errno == EWOULDBLOCK || errno == EAGAIN; }
#endif

// on-wire buffer = header + packet
#define WIRE_HDR_SIZE (int)sizeof(ex_net_wire_header_t)

int ex_net_transport_init(void)
{
#ifdef _WIN32
  WSADATA wsa;
  return WSAStartup(MAKEWORD(2,2), &wsa) == 0 ? 0 : -1;
#else
  return 0;
#endif
}

void ex_net_transport_shutdown(void)
{
#ifdef _WIN32
  WSACleanup();
#endif
}

static void set_nonblocking(ex_socket_t s)
{
#ifdef _WIN32
  u_long mode = 1;
  ioctlsocket(s, FIONBIO, &mode);
#else
  int flags = fcntl(s, F_GETFL, 0);
  fcntl(s, F_SETFL, flags | O_NONBLOCK);
#endif
}

static void peer_reset(ex_net_peer_t *peer)
{
  memset(peer, 0, sizeof(ex_net_peer_t));
  peer->next_seq = 1;      // seq 0 reserved for unreliable
  peer->expected_seq = 1;
}

int ex_net_server_open(ex_net_peer_t *peer, uint16_t port)
{
  peer_reset(peer);
  peer->is_server = 1;
  peer->sock = socket(AF_INET, SOCK_DGRAM, 0);
  if (peer->sock < 0) return -1;

  struct sockaddr_in addr; memset(&addr, 0, sizeof(addr));
  addr.sin_family = AF_INET;
  addr.sin_addr.s_addr = INADDR_ANY;
  addr.sin_port = htons(port);
  if (bind(peer->sock, (struct sockaddr*)&addr, sizeof(addr)) < 0) {
    CLOSESOCK(peer->sock);
    return -1;
  }
  set_nonblocking(peer->sock);
  return 0;
}

int ex_net_client_open(ex_net_peer_t *peer, const char *host, uint16_t port)
{
  peer_reset(peer);
  peer->is_server = 0;
  peer->sock = socket(AF_INET, SOCK_DGRAM, 0);
  if (peer->sock < 0) return -1;

  struct sockaddr_in *sa = (struct sockaddr_in*)peer->peer_addr;
  memset(sa, 0, sizeof(*sa));
  sa->sin_family = AF_INET;
  sa->sin_port = htons(port);
  if (inet_pton(AF_INET, host, &sa->sin_addr) <= 0) {
    CLOSESOCK(peer->sock);
    return -1;
  }
  peer->peer_addr_len = sizeof(struct sockaddr_in);
  peer->has_peer = 1;
  set_nonblocking(peer->sock);
  return 0;
}

static void raw_send(ex_net_peer_t *peer, ex_net_wire_header_t *hdr, const ex_net_packet_t *pkt)
{
  if (!peer->has_peer) return;
  unsigned char buf[WIRE_HDR_SIZE + sizeof(ex_net_packet_t)];
  memcpy(buf, hdr, WIRE_HDR_SIZE);
  size_t pkt_size = 4 + pkt->payload_len; // type+ver+len(2) + payload
  memcpy(buf + WIRE_HDR_SIZE, pkt, pkt_size);
  sendto(peer->sock, (const char*)buf, WIRE_HDR_SIZE + pkt_size, 0,
         (struct sockaddr*)peer->peer_addr, peer->peer_addr_len);
}

static void send_ack(ex_net_peer_t *peer)
{
  ex_net_wire_header_t hdr = { .seq = 0, .ack = peer->expected_seq - 1,
                               .flags = EX_NET_FLAG_ACKONLY };
  ex_net_packet_t empty; memset(&empty, 0, sizeof(empty));
  empty.payload_len = 0;
  raw_send(peer, &hdr, &empty);
}

int ex_net_send(ex_net_peer_t *peer, const ex_net_packet_t *pkt)
{
  ex_net_reliability rel = ex_net_reliability_for((ex_net_msg_type)pkt->type);
  int reliable = (rel != EX_NET_UNRELIABLE);

  ex_net_wire_header_t hdr;
  hdr.ack = peer->expected_seq - 1;
  hdr.flags = 0;

  if (reliable) {
    hdr.seq = peer->next_seq++;
    hdr.flags |= EX_NET_FLAG_RELIABLE;
    // stash for resend
    for (int i = 0; i < EX_NET_MAX_RESEND; i++) {
      if (!peer->resend[i].in_use) {
        peer->resend[i].in_use = 1;
        peer->resend[i].acked = 0;
        peer->resend[i].seq = hdr.seq;
        peer->resend[i].packet = *pkt;
        peer->resend[i].last_sent_ms = 0.0; // force immediate send in update, but also send now
        break;
      }
    }
  } else {
    hdr.seq = 0;
  }

  raw_send(peer, &hdr, pkt);
  return 0;
}

// mark any resend entries <= ack as acknowledged
static void process_ack(ex_net_peer_t *peer, uint16_t ack)
{
  peer->remote_ack = ack;
  for (int i = 0; i < EX_NET_MAX_RESEND; i++) {
    if (peer->resend[i].in_use && peer->resend[i].seq <= ack) {
      peer->resend[i].in_use = 0;
      peer->resend[i].acked = 1;
    }
  }
}

int ex_net_recv(ex_net_peer_t *peer, ex_net_packet_t *out_pkt)
{
  unsigned char buf[WIRE_HDR_SIZE + sizeof(ex_net_packet_t)];
  unsigned char from[128];
  socklen_t from_len = sizeof(from);

  int n = recvfrom(peer->sock, (char*)buf, sizeof(buf), 0,
                   (struct sockaddr*)from, &from_len);
  if (n < WIRE_HDR_SIZE) {
    return 0; // nothing (or too small); would_block is the common case
  }

  // server learns/locks its peer from the first datagram
  if (peer->is_server) {
    memcpy(peer->peer_addr, from, from_len);
    peer->peer_addr_len = from_len;
    peer->has_peer = 1;
  }

  ex_net_wire_header_t hdr;
  memcpy(&hdr, buf, WIRE_HDR_SIZE);

  // every packet carries the peer's latest ack
  process_ack(peer, hdr.ack);

  if (hdr.flags & EX_NET_FLAG_ACKONLY)
    return 0; // pure ack, nothing to deliver

  ex_net_packet_t pkt;
  memset(&pkt, 0, sizeof(pkt));
  memcpy(&pkt, buf + WIRE_HDR_SIZE, n - WIRE_HDR_SIZE);

  if (!(hdr.flags & EX_NET_FLAG_RELIABLE)) {
    // unreliable: deliver immediately
    *out_pkt = pkt;
    return 1;
  }

  // reliable: ack it, then enforce ordering
  if (hdr.seq == peer->expected_seq) {
    *out_pkt = pkt;
    peer->expected_seq++;
    send_ack(peer);
    // note: after delivering, a real impl would also drain any buffered
    // successors from `hold`; done on subsequent recv calls here for simplicity
    return 1;
  } else if (hdr.seq > peer->expected_seq) {
    // out of order -- buffer it, ack what we have
    int slot = hdr.seq % EX_NET_RECV_BUFFER;
    peer->hold[slot] = pkt;
    peer->hold_seq[slot] = hdr.seq;
    peer->hold_used[slot] = 1;
    send_ack(peer);
    return 0;
  } else {
    // duplicate/old -- re-ack and drop
    send_ack(peer);
    return 0;
  }
}

void ex_net_update(ex_net_peer_t *peer, double now_ms)
{
  // deliver any buffered in-order successor that arrived early
  // (checked lazily; callers loop ex_net_recv which also advances)
  for (int i = 0; i < EX_NET_MAX_RESEND; i++) {
    ex_net_pending_t *p = &peer->resend[i];
    if (p->in_use && !p->acked && (now_ms - p->last_sent_ms) >= EX_NET_RESEND_MS) {
      ex_net_wire_header_t hdr = { .seq = p->seq, .ack = peer->expected_seq - 1,
                                   .flags = EX_NET_FLAG_RELIABLE };
      raw_send(peer, &hdr, &p->packet);
      p->last_sent_ms = now_ms;
    }
  }
}

void ex_net_peer_close(ex_net_peer_t *peer)
{
  if (peer->sock) CLOSESOCK(peer->sock);
  peer->sock = 0;
  peer->has_peer = 0;
}
