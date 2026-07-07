#include "vk_backend.h"
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define VK_CHECK(x, msg) do { VkResult _r = (x); if (_r != VK_SUCCESS) { \
  fprintf(stderr, "[vk] %s failed (VkResult %d)\n", msg, _r); return (int)_r; } } while(0)

int ex_vk_create_instance(ex_vk_backend_t *vk, const char **instance_exts,
                           uint32_t instance_ext_cnt, int enable_validation)
{
  memset(vk, 0, sizeof(*vk));

  VkApplicationInfo app = { .sType = VK_STRUCTURE_TYPE_APPLICATION_INFO };
  app.pApplicationName = "Cybertron Arena";
  app.applicationVersion = VK_MAKE_VERSION(0, 1, 0);
  app.pEngineName = "exengine-vk";
  app.engineVersion = VK_MAKE_VERSION(0, 1, 0);
  app.apiVersion = VK_API_VERSION_1_1; // broadly supported on Mesa RADV + Asahi

  const char *layers[1];
  uint32_t layer_count = 0;
  if (enable_validation) {
    // only enable if the layer is actually present
    uint32_t avail = 0;
    vkEnumerateInstanceLayerProperties(&avail, NULL);
    VkLayerProperties *props = calloc(avail, sizeof(VkLayerProperties));
    vkEnumerateInstanceLayerProperties(&avail, props);
    for (uint32_t i = 0; i < avail; i++) {
      if (strcmp(props[i].layerName, "VK_LAYER_KHRONOS_validation") == 0) {
        layers[0] = "VK_LAYER_KHRONOS_validation";
        layer_count = 1;
        break;
      }
    }
    free(props);
  }

  VkInstanceCreateInfo ci = { .sType = VK_STRUCTURE_TYPE_INSTANCE_CREATE_INFO };
  ci.pApplicationInfo = &app;
  ci.enabledExtensionCount = instance_ext_cnt;
  ci.ppEnabledExtensionNames = instance_exts;
  ci.enabledLayerCount = layer_count;
  ci.ppEnabledLayerNames = layer_count ? layers : NULL;

  VK_CHECK(vkCreateInstance(&ci, NULL, &vk->instance), "vkCreateInstance");
  return 0;
}

static int pick_physical_device(ex_vk_backend_t *vk)
{
  uint32_t count = 0;
  vkEnumeratePhysicalDevices(vk->instance, &count, NULL);
  if (count == 0) { fprintf(stderr, "[vk] no Vulkan devices\n"); return -1; }

  VkPhysicalDevice *devs = calloc(count, sizeof(VkPhysicalDevice));
  vkEnumeratePhysicalDevices(vk->instance, &count, devs);

  // prefer a discrete GPU, else take the first that has a graphics+present queue
  VkPhysicalDevice chosen = VK_NULL_HANDLE;
  for (uint32_t i = 0; i < count; i++) {
    VkPhysicalDeviceProperties props;
    vkGetPhysicalDeviceProperties(devs[i], &props);

    uint32_t qcount = 0;
    vkGetPhysicalDeviceQueueFamilyProperties(devs[i], &qcount, NULL);
    VkQueueFamilyProperties *qs = calloc(qcount, sizeof(VkQueueFamilyProperties));
    vkGetPhysicalDeviceQueueFamilyProperties(devs[i], &qcount, qs);

    int gfx = -1, present = -1;
    for (uint32_t q = 0; q < qcount; q++) {
      if (qs[q].queueFlags & VK_QUEUE_GRAPHICS_BIT) gfx = q;
      VkBool32 supported = VK_FALSE;
      if (vk->surface)
        vkGetPhysicalDeviceSurfaceSupportKHR(devs[i], q, vk->surface, &supported);
      if (supported || !vk->surface) present = (present < 0 ? (int)q : present);
    }
    free(qs);

    if (gfx >= 0 && present >= 0) {
      vk->graphics_family = (uint32_t)gfx;
      vk->present_family  = (uint32_t)present;
      chosen = devs[i];
      if (props.deviceType == VK_PHYSICAL_DEVICE_TYPE_DISCRETE_GPU) break;
    }
  }
  free(devs);
  if (chosen == VK_NULL_HANDLE) return -1;
  vk->gpu = chosen;
  return 0;
}

static int create_logical_device(ex_vk_backend_t *vk)
{
  float prio = 1.0f;
  VkDeviceQueueCreateInfo qci[2];
  uint32_t qci_count = 1;

  qci[0] = (VkDeviceQueueCreateInfo){ .sType = VK_STRUCTURE_TYPE_DEVICE_QUEUE_CREATE_INFO };
  qci[0].queueFamilyIndex = vk->graphics_family;
  qci[0].queueCount = 1;
  qci[0].pQueuePriorities = &prio;

  if (vk->present_family != vk->graphics_family) {
    qci[1] = qci[0];
    qci[1].queueFamilyIndex = vk->present_family;
    qci_count = 2;
  }

  const char *dev_exts[] = { VK_KHR_SWAPCHAIN_EXTENSION_NAME };

  VkDeviceCreateInfo dci = { .sType = VK_STRUCTURE_TYPE_DEVICE_CREATE_INFO };
  dci.queueCreateInfoCount = qci_count;
  dci.pQueueCreateInfos = qci;
  dci.enabledExtensionCount = 1;
  dci.ppEnabledExtensionNames = dev_exts;

  VK_CHECK(vkCreateDevice(vk->gpu, &dci, NULL, &vk->device), "vkCreateDevice");
  vkGetDeviceQueue(vk->device, vk->graphics_family, 0, &vk->graphics_queue);
  vkGetDeviceQueue(vk->device, vk->present_family, 0, &vk->present_queue);
  return 0;
}

static int create_swapchain(ex_vk_backend_t *vk, uint32_t w, uint32_t h)
{
  VkSurfaceCapabilitiesKHR caps;
  vkGetPhysicalDeviceSurfaceCapabilitiesKHR(vk->gpu, vk->surface, &caps);

  uint32_t fmt_count = 0;
  vkGetPhysicalDeviceSurfaceFormatsKHR(vk->gpu, vk->surface, &fmt_count, NULL);
  VkSurfaceFormatKHR *fmts = calloc(fmt_count, sizeof(VkSurfaceFormatKHR));
  vkGetPhysicalDeviceSurfaceFormatsKHR(vk->gpu, vk->surface, &fmt_count, fmts);
  VkSurfaceFormatKHR chosen = fmts[0];
  for (uint32_t i = 0; i < fmt_count; i++)
    if (fmts[i].format == VK_FORMAT_B8G8R8A8_SRGB) { chosen = fmts[i]; break; }
  free(fmts);

  vk->swap_format = chosen.format;
  vk->swap_extent.width  = (caps.currentExtent.width  != UINT32_MAX) ? caps.currentExtent.width  : w;
  vk->swap_extent.height = (caps.currentExtent.height != UINT32_MAX) ? caps.currentExtent.height : h;

  uint32_t desired = caps.minImageCount + 1;
  if (caps.maxImageCount && desired > caps.maxImageCount) desired = caps.maxImageCount;

  VkSwapchainCreateInfoKHR sci = { .sType = VK_STRUCTURE_TYPE_SWAPCHAIN_CREATE_INFO_KHR };
  sci.surface = vk->surface;
  sci.minImageCount = desired;
  sci.imageFormat = chosen.format;
  sci.imageColorSpace = chosen.colorSpace;
  sci.imageExtent = vk->swap_extent;
  sci.imageArrayLayers = 1;
  sci.imageUsage = VK_IMAGE_USAGE_COLOR_ATTACHMENT_BIT;
  sci.preTransform = caps.currentTransform;
  sci.compositeAlpha = VK_COMPOSITE_ALPHA_OPAQUE_BIT_KHR;
  sci.presentMode = VK_PRESENT_MODE_FIFO_KHR; // always available (vsync)
  sci.clipped = VK_TRUE;

  uint32_t fams[2] = { vk->graphics_family, vk->present_family };
  if (vk->graphics_family != vk->present_family) {
    sci.imageSharingMode = VK_SHARING_MODE_CONCURRENT;
    sci.queueFamilyIndexCount = 2;
    sci.pQueueFamilyIndices = fams;
  } else {
    sci.imageSharingMode = VK_SHARING_MODE_EXCLUSIVE;
  }

  VK_CHECK(vkCreateSwapchainKHR(vk->device, &sci, NULL, &vk->swapchain), "vkCreateSwapchainKHR");

  vkGetSwapchainImagesKHR(vk->device, vk->swapchain, &vk->image_count, NULL);
  vk->images = calloc(vk->image_count, sizeof(VkImage));
  vkGetSwapchainImagesKHR(vk->device, vk->swapchain, &vk->image_count, vk->images);

  vk->image_views = calloc(vk->image_count, sizeof(VkImageView));
  for (uint32_t i = 0; i < vk->image_count; i++) {
    VkImageViewCreateInfo iv = { .sType = VK_STRUCTURE_TYPE_IMAGE_VIEW_CREATE_INFO };
    iv.image = vk->images[i];
    iv.viewType = VK_IMAGE_VIEW_TYPE_2D;
    iv.format = vk->swap_format;
    iv.subresourceRange.aspectMask = VK_IMAGE_ASPECT_COLOR_BIT;
    iv.subresourceRange.levelCount = 1;
    iv.subresourceRange.layerCount = 1;
    VK_CHECK(vkCreateImageView(vk->device, &iv, NULL, &vk->image_views[i]), "vkCreateImageView");
  }
  return 0;
}

static int create_render_pass(ex_vk_backend_t *vk)
{
  VkAttachmentDescription color = {0};
  color.format = vk->swap_format;
  color.samples = VK_SAMPLE_COUNT_1_BIT;
  color.loadOp = VK_ATTACHMENT_LOAD_OP_CLEAR;
  color.storeOp = VK_ATTACHMENT_STORE_OP_STORE;
  color.stencilLoadOp = VK_ATTACHMENT_LOAD_OP_DONT_CARE;
  color.stencilStoreOp = VK_ATTACHMENT_STORE_OP_DONT_CARE;
  color.initialLayout = VK_IMAGE_LAYOUT_UNDEFINED;
  color.finalLayout = VK_IMAGE_LAYOUT_PRESENT_SRC_KHR;

  VkAttachmentReference color_ref = { 0, VK_IMAGE_LAYOUT_COLOR_ATTACHMENT_OPTIMAL };
  VkSubpassDescription subpass = {0};
  subpass.pipelineBindPoint = VK_PIPELINE_BIND_POINT_GRAPHICS;
  subpass.colorAttachmentCount = 1;
  subpass.pColorAttachments = &color_ref;

  VkSubpassDependency dep = {0};
  dep.srcSubpass = VK_SUBPASS_EXTERNAL;
  dep.dstSubpass = 0;
  dep.srcStageMask = VK_PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT_BIT;
  dep.dstStageMask = VK_PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT_BIT;
  dep.dstAccessMask = VK_ACCESS_COLOR_ATTACHMENT_WRITE_BIT;

  VkRenderPassCreateInfo rp = { .sType = VK_STRUCTURE_TYPE_RENDER_PASS_CREATE_INFO };
  rp.attachmentCount = 1; rp.pAttachments = &color;
  rp.subpassCount = 1; rp.pSubpasses = &subpass;
  rp.dependencyCount = 1; rp.pDependencies = &dep;

  VK_CHECK(vkCreateRenderPass(vk->device, &rp, NULL, &vk->render_pass), "vkCreateRenderPass");

  vk->framebuffers = calloc(vk->image_count, sizeof(VkFramebuffer));
  for (uint32_t i = 0; i < vk->image_count; i++) {
    VkFramebufferCreateInfo fb = { .sType = VK_STRUCTURE_TYPE_FRAMEBUFFER_CREATE_INFO };
    fb.renderPass = vk->render_pass;
    fb.attachmentCount = 1;
    fb.pAttachments = &vk->image_views[i];
    fb.width = vk->swap_extent.width;
    fb.height = vk->swap_extent.height;
    fb.layers = 1;
    VK_CHECK(vkCreateFramebuffer(vk->device, &fb, NULL, &vk->framebuffers[i]), "vkCreateFramebuffer");
  }
  return 0;
}

static int create_commands_and_sync(ex_vk_backend_t *vk)
{
  VkCommandPoolCreateInfo cp = { .sType = VK_STRUCTURE_TYPE_COMMAND_POOL_CREATE_INFO };
  cp.flags = VK_COMMAND_POOL_CREATE_RESET_COMMAND_BUFFER_BIT;
  cp.queueFamilyIndex = vk->graphics_family;
  VK_CHECK(vkCreateCommandPool(vk->device, &cp, NULL, &vk->command_pool), "vkCreateCommandPool");

  VkCommandBufferAllocateInfo cba = { .sType = VK_STRUCTURE_TYPE_COMMAND_BUFFER_ALLOCATE_INFO };
  cba.commandPool = vk->command_pool;
  cba.level = VK_COMMAND_BUFFER_LEVEL_PRIMARY;
  cba.commandBufferCount = EX_VK_MAX_FRAMES_IN_FLIGHT;
  VK_CHECK(vkAllocateCommandBuffers(vk->device, &cba, vk->command_buffers), "vkAllocateCommandBuffers");

  VkSemaphoreCreateInfo sem = { .sType = VK_STRUCTURE_TYPE_SEMAPHORE_CREATE_INFO };
  VkFenceCreateInfo fence = { .sType = VK_STRUCTURE_TYPE_FENCE_CREATE_INFO,
                              .flags = VK_FENCE_CREATE_SIGNALED_BIT };
  for (int i = 0; i < EX_VK_MAX_FRAMES_IN_FLIGHT; i++) {
    VK_CHECK(vkCreateSemaphore(vk->device, &sem, NULL, &vk->image_available[i]), "sem");
    VK_CHECK(vkCreateSemaphore(vk->device, &sem, NULL, &vk->render_finished[i]), "sem");
    VK_CHECK(vkCreateFence(vk->device, &fence, NULL, &vk->in_flight[i]), "fence");
  }
  return 0;
}

int ex_vk_init_with_surface(ex_vk_backend_t *vk, VkSurfaceKHR surface,
                            uint32_t fb_width, uint32_t fb_height)
{
  vk->surface = surface;
  if (pick_physical_device(vk) != 0) { fprintf(stderr, "[vk] no suitable GPU\n"); return -1; }
  if (create_logical_device(vk) != 0) return -1;
  if (create_swapchain(vk, fb_width, fb_height) != 0) return -1;
  if (create_render_pass(vk) != 0) return -1;
  if (create_commands_and_sync(vk) != 0) return -1;
  vk->initialized = 1;
  return 0;
}

int ex_vk_draw_clear_frame(ex_vk_backend_t *vk, const float clear_rgba[4])
{
  if (!vk->initialized) return -1;
  uint32_t f = vk->current_frame;

  vkWaitForFences(vk->device, 1, &vk->in_flight[f], VK_TRUE, UINT64_MAX);

  uint32_t image_index;
  VkResult acq = vkAcquireNextImageKHR(vk->device, vk->swapchain, UINT64_MAX,
                                       vk->image_available[f], VK_NULL_HANDLE, &image_index);
  if (acq == VK_ERROR_OUT_OF_DATE_KHR) return 1; // caller should recreate swapchain
  vkResetFences(vk->device, 1, &vk->in_flight[f]);

  VkCommandBuffer cmd = vk->command_buffers[f];
  vkResetCommandBuffer(cmd, 0);
  VkCommandBufferBeginInfo bi = { .sType = VK_STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO };
  vkBeginCommandBuffer(cmd, &bi);

  VkClearValue clear;
  clear.color.float32[0] = clear_rgba[0];
  clear.color.float32[1] = clear_rgba[1];
  clear.color.float32[2] = clear_rgba[2];
  clear.color.float32[3] = clear_rgba[3];

  VkRenderPassBeginInfo rp = { .sType = VK_STRUCTURE_TYPE_RENDER_PASS_BEGIN_INFO };
  rp.renderPass = vk->render_pass;
  rp.framebuffer = vk->framebuffers[image_index];
  rp.renderArea.extent = vk->swap_extent;
  rp.clearValueCount = 1;
  rp.pClearValues = &clear;

  // --- this is where the deferred pipeline passes will go in the full renderer ---
  vkCmdBeginRenderPass(cmd, &rp, VK_SUBPASS_CONTENTS_INLINE);
  vkCmdEndRenderPass(cmd);
  vkEndCommandBuffer(cmd);

  VkPipelineStageFlags wait_stage = VK_PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT_BIT;
  VkSubmitInfo si = { .sType = VK_STRUCTURE_TYPE_SUBMIT_INFO };
  si.waitSemaphoreCount = 1;
  si.pWaitSemaphores = &vk->image_available[f];
  si.pWaitDstStageMask = &wait_stage;
  si.commandBufferCount = 1;
  si.pCommandBuffers = &cmd;
  si.signalSemaphoreCount = 1;
  si.pSignalSemaphores = &vk->render_finished[f];
  vkQueueSubmit(vk->graphics_queue, 1, &si, vk->in_flight[f]);

  VkPresentInfoKHR pi = { .sType = VK_STRUCTURE_TYPE_PRESENT_INFO_KHR };
  pi.waitSemaphoreCount = 1;
  pi.pWaitSemaphores = &vk->render_finished[f];
  pi.swapchainCount = 1;
  pi.pSwapchains = &vk->swapchain;
  pi.pImageIndices = &image_index;
  vkQueuePresentKHR(vk->present_queue, &pi);

  vk->current_frame = (f + 1) % EX_VK_MAX_FRAMES_IN_FLIGHT;
  return 0;
}

void ex_vk_shutdown(ex_vk_backend_t *vk)
{
  if (!vk->device) return;
  vkDeviceWaitIdle(vk->device);

  for (int i = 0; i < EX_VK_MAX_FRAMES_IN_FLIGHT; i++) {
    if (vk->image_available[i]) vkDestroySemaphore(vk->device, vk->image_available[i], NULL);
    if (vk->render_finished[i]) vkDestroySemaphore(vk->device, vk->render_finished[i], NULL);
    if (vk->in_flight[i]) vkDestroyFence(vk->device, vk->in_flight[i], NULL);
  }
  if (vk->command_pool) vkDestroyCommandPool(vk->device, vk->command_pool, NULL);
  if (vk->framebuffers)
    for (uint32_t i = 0; i < vk->image_count; i++)
      vkDestroyFramebuffer(vk->device, vk->framebuffers[i], NULL);
  if (vk->render_pass) vkDestroyRenderPass(vk->device, vk->render_pass, NULL);
  if (vk->image_views)
    for (uint32_t i = 0; i < vk->image_count; i++)
      vkDestroyImageView(vk->device, vk->image_views[i], NULL);
  if (vk->swapchain) vkDestroySwapchainKHR(vk->device, vk->swapchain, NULL);
  vkDestroyDevice(vk->device, NULL);
  if (vk->surface) vkDestroySurfaceKHR(vk->instance, vk->surface, NULL);
  if (vk->instance) vkDestroyInstance(vk->instance, NULL);

  free(vk->images); free(vk->image_views); free(vk->framebuffers);
  memset(vk, 0, sizeof(*vk));
}
