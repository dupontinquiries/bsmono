/* vk_backend
  Vulkan renderer bring-up scaffold for the Mesa/Asahi (aarch64) target.

  HONEST SCOPE: this is the *foundation* of the GL->Vulkan migration, not a
  finished renderer. It brings Vulkan up to the point of a clearing render
  pass: instance -> physical device selection -> logical device + queues ->
  surface -> swapchain -> render pass -> framebuffers -> per-frame command
  buffer that clears the swapchain image and presents. That's the milestone
  every Vulkan renderer is built on top of; exengine's actual deferred
  pipeline (g-buffer, lighting, shadows, SSAO, post) is the large remaining
  work that layers onto this.

  It's guarded behind the CMake option -DUSE_VULKAN=ON so the default build
  stays on the working GL path. It targets Vulkan 1.1 (widely supported on
  Mesa RADV and Asahi) and uses only core features -- no ray tracing (the
  Asahi/Mesa target doesn't expose hardware RT, per the design doc).

  Requires a VkSurface from the windowing layer. SDL2 (already used by
  exengine) can create one via SDL_Vulkan_CreateSurface; the caller passes
  it in so this file stays windowing-agnostic.
*/

#ifndef EX_VK_BACKEND_H
#define EX_VK_BACKEND_H

#include <vulkan/vulkan.h>
#include <stdint.h>

#define EX_VK_MAX_FRAMES_IN_FLIGHT 2

typedef struct {
  VkInstance        instance;
  VkPhysicalDevice  gpu;
  VkDevice          device;
  uint32_t          graphics_family;
  uint32_t          present_family;
  VkQueue           graphics_queue;
  VkQueue           present_queue;

  VkSurfaceKHR      surface;
  VkSwapchainKHR    swapchain;
  VkFormat          swap_format;
  VkExtent2D        swap_extent;
  uint32_t          image_count;
  VkImage          *images;
  VkImageView      *image_views;
  VkFramebuffer    *framebuffers;

  VkRenderPass      render_pass;
  VkCommandPool     command_pool;
  VkCommandBuffer   command_buffers[EX_VK_MAX_FRAMES_IN_FLIGHT];

  VkSemaphore       image_available[EX_VK_MAX_FRAMES_IN_FLIGHT];
  VkSemaphore       render_finished[EX_VK_MAX_FRAMES_IN_FLIGHT];
  VkFence           in_flight[EX_VK_MAX_FRAMES_IN_FLIGHT];
  uint32_t          current_frame;

  int               initialized;
} ex_vk_backend_t;

/**
 * [ex_vk_init bring Vulkan up through a clearing render pass]
 * @param vk               [backend to fill]
 * @param surface          [a VkSurfaceKHR created by the windowing layer]
 * @param fb_width         [initial framebuffer width]
 * @param fb_height        [initial framebuffer height]
 * @param instance_exts    [required instance extensions from the window (e.g. SDL_Vulkan_GetInstanceExtensions)]
 * @param instance_ext_cnt [count of the above]
 * @param enable_validation[1 to turn on VK_LAYER_KHRONOS_validation if present]
 * @return                 [0 on success, non-zero VkResult-ish on failure]
 *
 * NOTE: surface creation needs the instance, so the real integration is a
 * two-step handshake: ex_vk_create_instance() -> window makes surface ->
 * ex_vk_init_with_surface(). For the scaffold we expose the combined path
 * plus the instance-only helper below.
 */
int ex_vk_create_instance(ex_vk_backend_t *vk, const char **instance_exts,
                           uint32_t instance_ext_cnt, int enable_validation);

/**
 * [ex_vk_init_with_surface finish bring-up once a surface exists]
 */
int ex_vk_init_with_surface(ex_vk_backend_t *vk, VkSurfaceKHR surface,
                             uint32_t fb_width, uint32_t fb_height);

/**
 * [ex_vk_draw_clear_frame acquire -> clear -> present one frame]
 * Proves the swapchain + present loop works. The real renderer replaces
 * the clear-only command buffer with the deferred pipeline passes.
 * @param clear_rgba [clear color, 4 floats]
 */
int ex_vk_draw_clear_frame(ex_vk_backend_t *vk, const float clear_rgba[4]);

void ex_vk_shutdown(ex_vk_backend_t *vk);

#endif // EX_VK_BACKEND_H
