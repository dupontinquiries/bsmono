#include "netcode.h"
#include <string.h>

ex_net_reliability ex_net_reliability_for(ex_net_msg_type type)
{
  switch (type) {
    case EX_MSG_S_CHUNK_STATE:
    case EX_MSG_S_EVENT_FIRED:
    case EX_MSG_S_JOIN_ACCEPT:
    case EX_MSG_C_JOIN:
    case EX_MSG_C_TRANSFORM_REQUEST:
    case EX_MSG_C_DAMAGE_REQUEST:
      return EX_NET_RELIABLE_ORDERED;

    case EX_MSG_S_TRANSFORM_STATE:
      return EX_NET_RELIABLE;

    case EX_MSG_C_MOVE_INPUT:
    case EX_MSG_S_SNAPSHOT:
    default:
      return EX_NET_UNRELIABLE;
  }
}

// --- generic byte-buffer writer/reader helpers ---

static void put_u8(uint8_t *buf, size_t *off, uint8_t v)  { buf[(*off)++] = v; }
static void put_i32(uint8_t *buf, size_t *off, int32_t v) { memcpy(&buf[*off], &v, sizeof(v)); *off += sizeof(v); }
static void put_f32(uint8_t *buf, size_t *off, float v)   { memcpy(&buf[*off], &v, sizeof(v)); *off += sizeof(v); }
static void put_str(uint8_t *buf, size_t *off, const char *s, size_t max) {
  size_t len = strnlen(s, max - 1);
  memcpy(&buf[*off], s, len);
  memset(&buf[*off + len], 0, max - len);
  *off += max;
}

static uint8_t get_u8(const uint8_t *buf, size_t *off)  { return buf[(*off)++]; }
static int32_t get_i32(const uint8_t *buf, size_t *off) { int32_t v; memcpy(&v, &buf[*off], sizeof(v)); *off += sizeof(v); return v; }
static float   get_f32(const uint8_t *buf, size_t *off) { float v; memcpy(&v, &buf[*off], sizeof(v)); *off += sizeof(v); return v; }
static void    get_str(const uint8_t *buf, size_t *off, char *out, size_t max) {
  memcpy(out, &buf[*off], max);
  out[max - 1] = '\0';
  *off += max;
}

void ex_net_pack_chunk_state(ex_net_packet_t *pkt, const ex_net_chunk_state_t *data)
{
  pkt->type = EX_MSG_S_CHUNK_STATE;
  pkt->protocol_version = EX_NET_PROTOCOL_VERSION;

  size_t off = 0;
  put_str(pkt->payload, &off, data->chunk_name, sizeof(data->chunk_name));
  put_u8(pkt->payload, &off, data->new_state);
  put_f32(pkt->payload, &off, data->hp);

  pkt->payload_len = (uint16_t)off;
}

int ex_net_unpack_chunk_state(const ex_net_packet_t *pkt, ex_net_chunk_state_t *out)
{
  if (pkt->type != EX_MSG_S_CHUNK_STATE)
    return 0;

  size_t off = 0;
  get_str(pkt->payload, &off, out->chunk_name, sizeof(out->chunk_name));
  out->new_state = get_u8(pkt->payload, &off);
  out->hp = get_f32(pkt->payload, &off);

  return 1;
}

void ex_net_pack_event_fired(ex_net_packet_t *pkt, const ex_net_event_fired_t *data)
{
  pkt->type = EX_MSG_S_EVENT_FIRED;
  pkt->protocol_version = EX_NET_PROTOCOL_VERSION;

  size_t off = 0;
  put_str(pkt->payload, &off, data->event_name, sizeof(data->event_name));
  put_i32(pkt->payload, &off, data->int_arg);
  put_f32(pkt->payload, &off, data->float_arg);

  pkt->payload_len = (uint16_t)off;
}

int ex_net_unpack_event_fired(const ex_net_packet_t *pkt, ex_net_event_fired_t *out)
{
  if (pkt->type != EX_MSG_S_EVENT_FIRED)
    return 0;

  size_t off = 0;
  get_str(pkt->payload, &off, out->event_name, sizeof(out->event_name));
  out->int_arg = get_i32(pkt->payload, &off);
  out->float_arg = get_f32(pkt->payload, &off);

  return 1;
}

void ex_net_pack_transform_state(ex_net_packet_t *pkt, const ex_net_transform_state_t *data)
{
  pkt->type = EX_MSG_S_TRANSFORM_STATE;
  pkt->protocol_version = EX_NET_PROTOCOL_VERSION;

  size_t off = 0;
  put_i32(pkt->payload, &off, data->owner_id);
  put_u8(pkt->payload, &off, data->mode);
  put_u8(pkt->payload, &off, data->state);

  pkt->payload_len = (uint16_t)off;
}

int ex_net_unpack_transform_state(const ex_net_packet_t *pkt, ex_net_transform_state_t *out)
{
  if (pkt->type != EX_MSG_S_TRANSFORM_STATE)
    return 0;

  size_t off = 0;
  out->owner_id = get_i32(pkt->payload, &off);
  out->mode = get_u8(pkt->payload, &off);
  out->state = get_u8(pkt->payload, &off);

  return 1;
}
