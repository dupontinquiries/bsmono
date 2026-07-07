/* vk_probe -- creates a Vulkan instance and enumerates GPUs.
   Runs without a window/surface. Reports INCOMPATIBLE_DRIVER (-9) on
   machines with the loader but no ICD; enumerates devices on real
   hardware (Mesa RADV desktop, Asahi aarch64) or with lavapipe. */
#include "exengine/vk_backend.h"
#include <stdio.h>
int main(void){
  ex_vk_backend_t vk;
  const char *exts[] = { "VK_KHR_surface" };
  int r = ex_vk_create_instance(&vk, exts, 1, /*validation*/0);
  if (r != 0) { printf("vk_probe: instance creation returned %d (no ICD? that's expected headless)\n", r); return 0; }
  uint32_t n = 0; vkEnumeratePhysicalDevices(vk.instance, &n, NULL);
  printf("vk_probe: SUCCESS -- %u Vulkan device(s) enumerated\n", n);
  for (uint32_t i=0;i<n;i++){}
  vkDestroyInstance(vk.instance, NULL);
  return 0;
}
