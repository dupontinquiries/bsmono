#include "events.h"
#include <stdlib.h>
#include <string.h>
#include <stdio.h>

ex_event_bus_t* ex_event_bus_new(void)
{
  ex_event_bus_t *bus = malloc(sizeof(ex_event_bus_t));
  bus->listener_count = 0;
  return bus;
}

void ex_event_bus_free(ex_event_bus_t *bus)
{
  free(bus);
}

void ex_event_subscribe(ex_event_bus_t *bus, const char *event_name,
                         ex_event_callback callback, void *user_data)
{
  if (bus->listener_count >= EX_EVENT_MAX_LISTENERS) {
    fprintf(stderr, "[events] listener limit reached, ignoring subscribe for '%s'\n", event_name);
    return;
  }

  ex_event_listener_t *l = &bus->listeners[bus->listener_count++];
  strncpy(l->event_name, event_name, EX_EVENT_NAME_MAX - 1);
  l->callback = callback;
  l->user_data = user_data;
}

void ex_event_fire(ex_event_bus_t *bus, const char *event_name, int int_arg, float float_arg)
{
  ex_event_t event;
  strncpy(event.name, event_name, EX_EVENT_NAME_MAX - 1);
  event.int_arg = int_arg;
  event.float_arg = float_arg;

  for (int i = 0; i < bus->listener_count; i++) {
    if (strncmp(bus->listeners[i].event_name, event_name, EX_EVENT_NAME_MAX) == 0)
      bus->listeners[i].callback(&event, bus->listeners[i].user_data);
  }
}
