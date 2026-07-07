/* events
  Minimal publish/subscribe event bus for map-state triggers
  (airship destroyed, capture point held, late-match escalation, etc).

  Does not exist in stock exengine -- util/entity.c has basic
  entities but nothing event-driven. This is intentionally tiny:
  it's a dispatch mechanism, not a full ECS.

  Server-authoritative usage pattern: game logic on the
  authoritative host calls ex_event_fire(); listeners (chunk
  world, hazard volumes, UI) react locally. In multiplayer this
  fire call itself should be the thing replicated to clients,
  not the downstream state changes it causes.
*/

#ifndef EX_EVENTS_H
#define EX_EVENTS_H

#include <stdint.h>

#define EX_EVENT_NAME_MAX 48
#define EX_EVENT_MAX_LISTENERS 256

typedef struct {
  char name[EX_EVENT_NAME_MAX];  // e.g. "airship_destroyed"
  int int_arg;                    // small payload, e.g. lane id / team id
  float float_arg;                // e.g. score threshold value
} ex_event_t;

typedef void (*ex_event_callback)(const ex_event_t *event, void *user_data);

typedef struct {
  char event_name[EX_EVENT_NAME_MAX];
  ex_event_callback callback;
  void *user_data;
} ex_event_listener_t;

typedef struct {
  ex_event_listener_t listeners[EX_EVENT_MAX_LISTENERS];
  int listener_count;
} ex_event_bus_t;

/**
 * [ex_event_bus_new create an empty bus]
 */
ex_event_bus_t* ex_event_bus_new(void);

/**
 * [ex_event_bus_free release the bus]
 */
void ex_event_bus_free(ex_event_bus_t *bus);

/**
 * [ex_event_subscribe register a callback for a named event]
 * @param bus        [the event bus]
 * @param event_name [name to listen for, e.g. "airship_destroyed"]
 * @param callback   [called synchronously when the event fires]
 * @param user_data  [opaque pointer passed back to the callback, e.g. a chunk*]
 */
void ex_event_subscribe(ex_event_bus_t *bus, const char *event_name,
                         ex_event_callback callback, void *user_data);

/**
 * [ex_event_fire dispatch an event to all matching listeners]
 * Call only from authoritative game logic in multiplayer contexts.
 */
void ex_event_fire(ex_event_bus_t *bus, const char *event_name, int int_arg, float float_arg);

#endif // EX_EVENTS_H
