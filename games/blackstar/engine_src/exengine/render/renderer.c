#include "render.h"
#include "math/mathlib.h"

// projections
mat4x4 projection_90deg;

// shaders
GLuint pointfbo_shader    = 0;
GLuint forward_shader     = 0;
/* SUN SHADOW MAP: one orthographic cascade following the camera, rendered along the
   live planetary sun direction each frame (quality high+). */
GLuint sunshadow_shader   = 0;
GLuint sun_shadow_fbo = 0, sun_shadow_tex = 0;
GLuint canvas_depth_tex = 0;   /* the canvas depth as a TEXTURE: SSAO + depth-aware post */
#define SUN_SHADOW_RES 2048
mat4x4 sun_light_mat;
GLuint framebuffer_shader = 0;

// buffers
GLuint screenquad_vao, screenquad_vbo;

// framebuffers
typedef struct {
  GLuint fbo, rbo, cbo;
  GLuint vao, vbo;
  size_t width, height;
} ex_framebuffer_t;
ex_framebuffer_t framebuffer;

// some default textures
GLuint default_texture_diffuse;
GLuint default_texture_normal;
GLuint default_texture_specular;
GLuint default_texture_emission;

void ex_render_init()
{
  /* -- SHADERS -- */
  pointfbo_shader  = ex_shader("pointfbo.glsl");
  framebuffer_shader = ex_shader("fboshader.glsl");
  sunshadow_shader   = ex_shader("sunshadow.glsl");
  { /* the sun's depth target: outside the map reads as depth 1.0 (fully lit) */
    glGenFramebuffers(1, &sun_shadow_fbo);
    glGenTextures(1, &sun_shadow_tex);
    glBindTexture(GL_TEXTURE_2D, sun_shadow_tex);
    glTexImage2D(GL_TEXTURE_2D, 0, GL_DEPTH_COMPONENT24, SUN_SHADOW_RES, SUN_SHADOW_RES, 0, GL_DEPTH_COMPONENT, GL_FLOAT, NULL);
    glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_NEAREST);
    glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_NEAREST);
    glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_S, GL_CLAMP_TO_BORDER);
    glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_T, GL_CLAMP_TO_BORDER);
    float bcol[4] = {1.0f,1.0f,1.0f,1.0f};
    glTexParameterfv(GL_TEXTURE_2D, GL_TEXTURE_BORDER_COLOR, bcol);
    glBindFramebuffer(GL_FRAMEBUFFER, sun_shadow_fbo);
    glFramebufferTexture2D(GL_FRAMEBUFFER, GL_DEPTH_ATTACHMENT, GL_TEXTURE_2D, sun_shadow_tex, 0);
    glDrawBuffer(GL_NONE); glReadBuffer(GL_NONE);
    glBindFramebuffer(GL_FRAMEBUFFER, 0);
  }

  // set up a 90 degree projection
  // mostly for omni-directional lights
  float aspect = (float)SHADOW_MAP_SIZE/(float)SHADOW_MAP_SIZE;
  mat4x4_perspective(projection_90deg, rad(90.0f), aspect, 0.1f, EX_POINT_FAR_PLANE);
  /* ------------- */


  /* -- DEFAULTS -- */
  GLuint *textures[] = {
    &default_texture_diffuse,
    &default_texture_normal,
    &default_texture_specular,
    &default_texture_emission
  };

  char colors[][4] = {
    {0,0,0,255},
    {128,127,255,255},
    {0,0,0,255},
    {0,0,0,255}
  };

  for (int i=0; i<4; i++) {
    glGenTextures(1, textures[i]);
    glBindTexture(GL_TEXTURE_2D, *textures[i]);
    glTexImage2D(GL_TEXTURE_2D, 0, GL_RGBA, 1, 1, 0, GL_RGBA, GL_UNSIGNED_BYTE, &colors[i][0]);
  }
  glBindTexture(GL_TEXTURE_2D, 0);
  /* -------------- */


  /* -- SCREEN QUAD -- */
  GLfloat vertices[] = {
    // pos         // uv
    -1.0f,  1.0f,  0.0f, 1.0f,
    -1.0f, -1.0f,  0.0f, 0.0f,
     1.0f, -1.0f,  1.0f, 0.0f,
    -1.0f,  1.0f,  0.0f, 1.0f,
     1.0f, -1.0f,  1.0f, 0.0f,
     1.0f,  1.0f,  1.0f, 1.0f
  };

  glGenVertexArrays(1, &screenquad_vao);
  glGenBuffers(1, &screenquad_vbo);
  glBindVertexArray(screenquad_vao);

  // vertices
  glBindBuffer(GL_ARRAY_BUFFER, screenquad_vbo);
  glBufferData(GL_ARRAY_BUFFER, sizeof(vertices), &vertices[0], GL_STATIC_DRAW);

  // position
  glVertexAttribPointer(0, 2, GL_FLOAT, GL_FALSE, sizeof(GLfloat)*4, (GLvoid*)0);
  glEnableVertexAttribArray(0);

  // tex coords
  glVertexAttribPointer(1, 2, GL_FLOAT, GL_FALSE, sizeof(GLfloat)*4, (GLvoid*)(2 * sizeof(GLfloat)));
  glEnableVertexAttribArray(1);
  glBindVertexArray(0);
  /* ----------------- */


  /* -- FRAMEBUFFER -- */
  glGenFramebuffers(1, &framebuffer.fbo);
  glGenRenderbuffers(1, &framebuffer.rbo);
  glGenTextures(1, &framebuffer.cbo);
  framebuffer.vao = screenquad_vao;
  framebuffer.vbo = screenquad_vbo;

  ex_render_resize(display.width, display.height);
  /* ----------------- */
}

void ex_render(ex_renderer_e renderer, ex_renderable_t *renderables)
{
  /* -- LIGHT DEPTH MAPS -- */
  ex_renderlist_t *point_lights = &renderables->point_lights;
  ex_renderlist_t *models       = &renderables->models;

  // render light depth maps
  glCullFace(GL_BACK);
  ex_shader_use(pointfbo_shader);
  for (int i=0; i<point_lights->count; i++) {
    ex_point_light_t *light = (ex_point_light_t*)point_lights->nodes[i].obj;

    if ((light->dynamic || light->update) && light->is_shadow && light->is_visible) {
      ex_render_point_light_begin(light, pointfbo_shader);

      // render all shadow-casting models
      for (int j=0; j<models->count; j++) {
        ex_model_t *model = (ex_model_t*)models->nodes[j].obj;

        if (model->is_shadow)
          ex_render_model(model, pointfbo_shader);
      }
    }

  }
  /* ---------------------- */

  /* -- SUN SHADOW PASS (high+): one ortho cascade centred ahead of the camera -- */
  if (ex_gfx_quality >= 1.5f && ex_sun_strength > 0.03f && renderables->camera) {
    {
      ex_camera_matrices_t *scam = renderables->camera;
      /* camera world position = inverse_view translation; forward = -Z axis of it */
      float cpx = scam->inverse_view[3][0], cpy = scam->inverse_view[3][1], cpz = scam->inverse_view[3][2];
      float cfx = -scam->inverse_view[2][0], cfz = -scam->inverse_view[2][2];
      vec3 sd = { ex_sun_dir[0], ex_sun_dir[1], ex_sun_dir[2] };
      float sl = sqrtf(sd[0]*sd[0]+sd[1]*sd[1]+sd[2]*sd[2]); if (sl < 0.001f){ sd[0]=0.35f; sd[1]=0.9f; sd[2]=0.35f; sl=1.0f; }
      vec3 center = { cpx + cfx*45.0f, cpy, cpz + cfz*45.0f };
      vec3 eye = { center[0] + sd[0]/sl*320.0f, center[1] + sd[1]/sl*320.0f, center[2] + sd[2]/sl*320.0f };
      vec3 up = {0.0f, 1.0f, 0.0f};
      mat4x4 lview, lproj;
      mat4x4_look_at(lview, eye, center, up);
      mat4x4_ortho(lproj, -150.0f, 150.0f, -150.0f, 150.0f, 1.0f, 800.0f);
      mat4x4_mul(sun_light_mat, lproj, lview);
      glViewport(0, 0, SUN_SHADOW_RES, SUN_SHADOW_RES);
      glBindFramebuffer(GL_FRAMEBUFFER, sun_shadow_fbo);
      glClear(GL_DEPTH_BUFFER_BIT);
      glEnable(GL_CULL_FACE); glCullFace(GL_FRONT);   /* front-culling fights acne */
      ex_shader_use(sunshadow_shader);
      glUniformMatrix4fv(ex_uniform(sunshadow_shader, "u_light_mat"), 1, GL_FALSE, sun_light_mat[0]);
      for (int j=0; j<models->count; j++) {
        ex_model_t *model = (ex_model_t*)models->nodes[j].obj;
        if (model->is_shadow) ex_render_model(model, sunshadow_shader);
      }
      glCullFace(GL_BACK);
      glBindFramebuffer(GL_FRAMEBUFFER, 0);
    }
  }
  /* ----------------------------------------------------------------------- */

  /* -- RENDER PASS -- */
  switch (renderer) {
    case EX_RENDERER_FORWARD: {
      ex_render_forward(renderables);
      break;
    }
    case EX_RENDERER_DEFERRED: {
      break;
    }
  }
  /* ----------------- */
}

float ex_clear_color[3] = {0.0f,0.0f,0.0f};
float ex_fog_color[3] = {0.02f,0.02f,0.04f};
float ex_fog_density = 0.010f;
float ex_gfx_quality = 2.0f;   /* 0 mobile .. 3 ultra; scales shader effect cost */
float ex_render_time = 0.0f;   /* seconds; drives animated procedural ground */
/* PLANETARY SUN: direction/strength/colour driven by the game's day-cycle system.
   Defaults reproduce the old hardcoded look exactly. */
float ex_sun_dir[3]    = { 0.35f, 0.9f, 0.35f };
float ex_sun_strength  = 0.62f;
float ex_sun_color[3]  = { 1.0f, 1.0f, 1.0f };
float ex_wetness       = 0.0f;   /* 0 dry .. 1 soaked: boosts env-reflection + glint */
float ex_sun_uv[2]     = { 0.5f, 0.5f };   /* the sun's screen position (0..1), for the post lens flare */
float ex_sun_vis       = 0.0f;             /* 0 offscreen/below .. 1 full flare */
float ex_grade_warm[3] = { 1.0f, 0.92f, 0.80f };  /* highlight tint (per-world cinematography) */
float ex_grade_cool[3] = { 0.78f, 0.86f, 1.0f };  /* shadow tint */
float ex_cloud_amt     = 0.0f;   /* 0 clear/interior .. 1 heavy drifting cloud shadow */
float ex_exposure      = 1.0f;   /* eye ADAPTATION: >1 = pupils open (night), lags behind the light */
float ex_render_scale = 1.0f;  /* internal render resolution factor (0.5 .. 1.0) */
int   ex_shadow_map_size = SHADOW_MAP_SIZE;  /* runtime shadow cubemap res (perf lever) */
void ex_render_forward(ex_renderable_t *renderables)
{
  /* INIT SOME STUFF */
  // render lists
  ex_renderlist_t *point_lights = &renderables->point_lights;
  ex_renderlist_t *models       = &renderables->models;

  // bind main framebuffer
  glViewport(0, 0, framebuffer.width, framebuffer.height);
  glBindFramebuffer(GL_FRAMEBUFFER, framebuffer.fbo);
  glClearColor(ex_clear_color[0], ex_clear_color[1], ex_clear_color[2], 1.0f);
  glClear(GL_COLOR_BUFFER_BIT | GL_DEPTH_BUFFER_BIT);
  glEnable(GL_DEPTH_TEST);
  // Backface culling disabled: procedurally-generated meshes (character parts,
  // map props, destructibles) have mixed triangle winding, which culling turns
  // into missing faces. Rendering both sides fixes that; we have FPS to spare.
  glDisable(GL_CULL_FACE);

  if (!forward_shader)
    forward_shader = ex_shader("forward.glsl");
  /* --------------- */


  /* FIRST PASS */
  ex_shader_use(forward_shader);

  // send camera vars to shader
  ex_camera_matrices_t *camera = renderables->camera;
  glUniformMatrix4fv(ex_uniform(forward_shader, "u_projection"), 1, GL_FALSE, camera->projection[0]);
  glUniformMatrix4fv(ex_uniform(forward_shader, "u_view"), 1, GL_FALSE, camera->view[0]);
  { /* project the planetary sun for the post-pass LENS FLARE: a far point along the
       sun direction through view+projection -> screen uv + a visibility factor that
       fades at the screen edges and dies when the sun is behind or below. */
    float sd[3]={ex_sun_dir[0],ex_sun_dir[1],ex_sun_dir[2]};
    float sl=sqrtf(sd[0]*sd[0]+sd[1]*sd[1]+sd[2]*sd[2]); if(sl<0.001f){sd[0]=0.35f;sd[1]=0.9f;sd[2]=0.35f;sl=1.0f;}
    float cpx2=camera->inverse_view[3][0], cpy2=camera->inverse_view[3][1], cpz2=camera->inverse_view[3][2];
    float wp[4]={cpx2+sd[0]/sl*2000.0f, cpy2+sd[1]/sl*2000.0f, cpz2+sd[2]/sl*2000.0f, 1.0f};
    float vp4[4], cp4[4];
    for(int r4=0;r4<4;r4++) vp4[r4]=camera->view[0][r4]*wp[0]+camera->view[1][r4]*wp[1]+camera->view[2][r4]*wp[2]+camera->view[3][r4]*wp[3];
    for(int r4=0;r4<4;r4++) cp4[r4]=camera->projection[0][r4]*vp4[0]+camera->projection[1][r4]*vp4[1]+camera->projection[2][r4]*vp4[2]+camera->projection[3][r4]*vp4[3];
    if (cp4[3] > 0.001f){
      float sx=cp4[0]/cp4[3]*0.5f+0.5f, sy2=cp4[1]/cp4[3]*0.5f+0.5f;
      ex_sun_uv[0]=sx; ex_sun_uv[1]=sy2;
      float edge = fminf(fminf(sx,1.0f-sx), fminf(sy2,1.0f-sy2));         /* 0 at edge */
      float ev = fmaxf(0.0f, fminf(1.0f, edge*4.0f));
      float elv = fmaxf(0.0f, fminf(1.0f, (sd[1]/sl)*6.0f));              /* dies below horizon */
      ex_sun_vis = ev*elv*fminf(1.0f, ex_sun_strength/0.35f);
    } else ex_sun_vis = 0.0f;
  }
  glUniformMatrix4fv(ex_uniform(forward_shader, "u_inverse_view"), 1, GL_FALSE, camera->inverse_view[0]);
  glUniform3fv(ex_uniform(forward_shader, "u_fog_color"), 1, ex_fog_color);
  glUniform1f(ex_uniform(forward_shader, "u_fog_density"), ex_fog_density);
  glUniform1f(ex_uniform(forward_shader, "u_gfx_quality"), ex_gfx_quality);
  glUniform1f(ex_uniform(forward_shader, "u_time"), ex_render_time);
  glUniform3fv(ex_uniform(forward_shader, "u_sun_dir"), 1, ex_sun_dir);
  glUniform1f(ex_uniform(forward_shader, "u_sun_strength"), ex_sun_strength);
  glUniformMatrix4fv(ex_uniform(forward_shader, "u_sun_light_mat"), 1, GL_FALSE, sun_light_mat[0]);
  glUniform1i(ex_uniform(forward_shader, "u_sun_shadow_on"), (ex_gfx_quality >= 1.5f && ex_sun_strength > 0.03f) ? 1 : 0);
  glActiveTexture(GL_TEXTURE9);
  glBindTexture(GL_TEXTURE_2D, sun_shadow_tex);
  glUniform1i(ex_uniform(forward_shader, "u_sun_shadow"), 9);
  glUniform3fv(ex_uniform(forward_shader, "u_sun_color"), 1, ex_sun_color);
  glUniform1f(ex_uniform(forward_shader, "u_wetness"), ex_wetness);
  glUniform1f(ex_uniform(forward_shader, "u_cloud_amt"), ex_cloud_amt);

  // first pass is ambient
  glDisable(GL_BLEND);
  glCullFace(GL_BACK);

  // do all non shadow casting lights in a single pass
  char buff[64];
  size_t pcount = 0;
  for (int i=0; i<point_lights->count; i++) {
    ex_point_light_t *light = (ex_point_light_t*)point_lights->nodes[i].obj;

    if (light->is_visible && !light->is_shadow) {
      sprintf(buff, "u_point_lights[%u]", (uint32_t)pcount++);
      ex_render_point_light(light, forward_shader, buff);
    }
  }

  glUniform1i(ex_uniform(forward_shader, "u_point_active"), 0);
  glUniform1i(ex_uniform(forward_shader, "u_point_count"), pcount);
  glUniform1i(ex_uniform(forward_shader, "u_ambient_pass"), 1);
  for (int i=0; i<models->count; i++) {
    ex_model_t *model = (ex_model_t*)models->nodes[i].obj;
    ex_render_model(model, forward_shader);
  }
  glUniform1i(ex_uniform(forward_shader, "u_ambient_pass"), 0);
  glUniform1i(ex_uniform(forward_shader, "u_point_count"), 0);
  /* ------------ */


  /* SECOND PASS */
  // enable blending for second pass onwards
  glEnable(GL_BLEND);
  glBlendFunc(GL_SRC_ALPHA, GL_ONE);

  // render all shadow casting point lights
  glUniform1i(ex_uniform(forward_shader, "u_point_active"), 1);
  for (int i=0; i<point_lights->count; i++) {
    ex_point_light_t *light = (ex_point_light_t*)point_lights->nodes[i].obj;

    if (!light->is_shadow || !light->is_visible)
      continue;

    ex_render_point_light(light, forward_shader, NULL);
    for (int i=0; i<models->count; i++) {
      ex_model_t *model = (ex_model_t*)models->nodes[i].obj;
      ex_render_model(model, forward_shader);
    }
  }
  glDisable(GL_BLEND);
  glBindFramebuffer(GL_FRAMEBUFFER, 0);
  /* ----------- */


  /* RENDER TO SCREEN */
  glViewport(0, 0, display.width, display.height);
  glClearColor(0.0f, 0.0f, 0.0f, 1.0f);
  glClear(GL_COLOR_BUFFER_BIT | GL_DEPTH_BUFFER_BIT);   // never composite over stale frame content
  glDisable(GL_DEPTH_TEST);
  glEnable(GL_FRAMEBUFFER_SRGB);

  ex_shader_use(framebuffer_shader);

  glBindVertexArray(framebuffer.vao);
  glActiveTexture(GL_TEXTURE0);
  glUniform1i(ex_uniform(framebuffer_shader, "u_texture"), 0);
  glActiveTexture(GL_TEXTURE1);
  glBindTexture(GL_TEXTURE_2D, canvas_depth_tex);
  glUniform1i(ex_uniform(framebuffer_shader, "u_depth"), 1);
  glActiveTexture(GL_TEXTURE0);
  glUniform1f(ex_uniform(framebuffer_shader, "u_gfx_quality"), ex_gfx_quality);
  glUniform2fv(ex_uniform(framebuffer_shader, "u_sun_uv"), 1, ex_sun_uv);
  glUniform1f(ex_uniform(framebuffer_shader, "u_sun_vis"), ex_sun_vis);
  glUniform3fv(ex_uniform(framebuffer_shader, "u_grade_warm"), 1, ex_grade_warm);
  glUniform3fv(ex_uniform(framebuffer_shader, "u_grade_cool"), 1, ex_grade_cool);
  glUniform1f(ex_uniform(framebuffer_shader, "u_time"), ex_render_time);
  glUniform1f(ex_uniform(framebuffer_shader, "u_exposure"), ex_exposure);
  glUniform1f(ex_uniform(framebuffer_shader, "u_upscale"), (ex_render_scale > 0.05f) ? 1.0f/ex_render_scale : 1.0f);
  glBindTexture(GL_TEXTURE_2D, framebuffer.cbo);
  glDrawArrays(GL_TRIANGLES, 0, 6);
  glBindVertexArray(0);
  glBindTexture(GL_TEXTURE_2D, 0);
  /* ---------------- */
}

void ex_render_model(ex_model_t *model, GLuint shader)
{
  // handle transformations
  if (!model->use_transform && model->instance_count < 2) {
    mat4x4_identity(model->transforms[0]);
    mat4x4_translate_in_place(model->transforms[0], model->position[0], model->position[1], model->position[2]);
    mat4x4_rotate_Y(model->transforms[0], model->transforms[0], rad(model->rotation[1]));
    mat4x4_rotate_X(model->transforms[0], model->transforms[0], rad(model->rotation[0]));
    mat4x4_rotate_Z(model->transforms[0], model->transforms[0], rad(model->rotation[2]));
    if (model->scale3[0]!=0.0f || model->scale3[1]!=0.0f || model->scale3[2]!=0.0f)
      mat4x4_scale_aniso(model->transforms[0], model->transforms[0], model->scale3[0], model->scale3[1], model->scale3[2]);
    else
      mat4x4_scale_aniso(model->transforms[0], model->transforms[0], model->scale, model->scale, model->scale);
  }

  // pass bone data
  GLuint has_skeleton_loc = ex_uniform(shader, "u_has_skeleton");
  glUniform1i(has_skeleton_loc, 0);

  if (model->bones != NULL && model->current_anim != NULL) {
    glUniform1i(has_skeleton_loc, 1);
    glUniformMatrix4fv(ex_uniform(shader, "u_bone_matrix"), model->bones_len, GL_TRUE, &model->skeleton[0][0][0]);
  }

  // update instancing matrix vbo
  if (!model->is_static || model->is_static == 1) {
    glBindBuffer(GL_ARRAY_BUFFER, model->instance_vbo);
    GLvoid *ptr = glMapBuffer(GL_ARRAY_BUFFER, GL_WRITE_ONLY);
    memcpy(ptr, &model->transforms[0], model->instance_count * sizeof(mat4x4));
    glUnmapBuffer(GL_ARRAY_BUFFER);

    if (model->is_static)
      model->is_static = 2;
  }

  // these remain the same for each mesh
  glUniform1i(ex_uniform(shader, "u_texture"), 4);
  glUniform1i(ex_uniform(shader, "u_spec"), 5);
  glUniform1i(ex_uniform(shader, "u_norm"), 6);
  glUniform1i(ex_uniform(shader, "u_emission"), 8);

  // render meshes
  for (int i=0; i<EX_MODEL_MAX_MESHES; i++) {
    if (model->meshes[i] == NULL)
      continue;

    glUniform1i(ex_uniform(shader, "u_is_lit"), model->is_lit);
    glUniform1i(ex_uniform(shader, "u_emissive_solid"), model->emissive_solid);
    glUniform3fv(ex_uniform(shader, "u_tint"), 1, model->tint);
    glUniform1f(ex_uniform(shader, "u_emission_strength"), model->emission_strength>0.0001f?model->emission_strength:1.0f);
    glUniform1f(ex_uniform(shader, "u_mat_boost"), model->mat_boost>0.0001f?model->mat_boost:1.0f);
    ex_render_mesh(model->meshes[i], shader, model->instance_count);
  }
}

void ex_render_mesh(ex_mesh_t *mesh, GLuint shader, size_t count)
{
  // bind vao/ebo/tex
  glBindVertexArray(mesh->VAO);
  glBindBuffer(GL_ELEMENT_ARRAY_BUFFER, mesh->EBO);

  // diffuse
  glActiveTexture(GL_TEXTURE4);
  if (mesh->texture < 1)
    glBindTexture(GL_TEXTURE_2D, default_texture_diffuse);
  else
    glBindTexture(GL_TEXTURE_2D, mesh->texture);

  // specular
  glActiveTexture(GL_TEXTURE5);
  if (mesh->texture_spec < 1)
    glBindTexture(GL_TEXTURE_2D, default_texture_specular);
  else
    glBindTexture(GL_TEXTURE_2D, mesh->texture_spec);

  // normal
  glActiveTexture(GL_TEXTURE6);
  if (mesh->texture_norm < 1)
    glBindTexture(GL_TEXTURE_2D, default_texture_normal);
  else
    glBindTexture(GL_TEXTURE_2D, mesh->texture_norm);

  // emission map (unit 8): additive self-lit detail; default black = none
  glActiveTexture(GL_TEXTURE8);
  if (mesh->texture_emission < 1)
    glBindTexture(GL_TEXTURE_2D, default_texture_emission);
  else
    glBindTexture(GL_TEXTURE_2D, mesh->texture_emission);

  // draw mesh
  glDrawElementsInstanced(GL_TRIANGLES, mesh->icount, GL_UNSIGNED_INT, 0, count);

  // unbind buffers
  glBindTexture(GL_TEXTURE_2D, 0);
  glBindBuffer(GL_ELEMENT_ARRAY_BUFFER, 0);
  glBindVertexArray(0);
}

void ex_render_point_light_begin(ex_point_light_t *light, GLuint shader)
{
  // prevent needless updates
  light->update = 0;

  // set up light transforms for cube map
  vec3 temp;
  vec3_add(temp, light->position, (vec3){1.0f, 0.0f, 0.0f});
  mat4x4_look_at(light->transform[0], light->position, temp, (vec3){0.0f, -1.0f, 0.0f});
  mat4x4_mul(light->transform[0], projection_90deg, light->transform[0]);

  vec3_add(temp, light->position, (vec3){-1.0f, 0.0f, 0.0f});
  mat4x4_look_at(light->transform[1], light->position, temp, (vec3){0.0f, -1.0f, 0.0f});
  mat4x4_mul(light->transform[1], projection_90deg, light->transform[1]);

  vec3_add(temp, light->position, (vec3){0.0f, 1.0f, 0.0f});
  mat4x4_look_at(light->transform[2], light->position, temp, (vec3){0.0f, 0.0f, 1.0f});
  mat4x4_mul(light->transform[2], projection_90deg, light->transform[2]);

  vec3_add(temp, light->position, (vec3){0.0f, -1.0f, 0.0f});
  mat4x4_look_at(light->transform[3], light->position, temp, (vec3){0.0f, 0.0f, -1.0f});
  mat4x4_mul(light->transform[3], projection_90deg, light->transform[3]);

  vec3_add(temp, light->position, (vec3){0.0f, 0.0f, 1.0f});
  mat4x4_look_at(light->transform[4], light->position, temp, (vec3){0.0f, -1.0f, 0.0f});
  mat4x4_mul(light->transform[4], projection_90deg, light->transform[4]);

  vec3_add(temp, light->position, (vec3){0.0f, 0.0f, -1.0f});
  mat4x4_look_at(light->transform[5], light->position, temp, (vec3){0.0f, -1.0f, 0.0f});
  mat4x4_mul(light->transform[5], projection_90deg, light->transform[5]);

  // render to depth cube map
  glViewport(0, 0, ex_shadow_map_size, ex_shadow_map_size);
  glBindFramebuffer(GL_FRAMEBUFFER, light->depth_map_fbo);

  glClear(GL_DEPTH_BUFFER_BIT);
  glEnable(GL_DEPTH_TEST);

  // pass transform matrices to shader
  glUniformMatrix4fv(ex_uniform(shader, "u_shadow_matrices[0]"), 1, GL_FALSE, *light->transform[0]);
  glUniformMatrix4fv(ex_uniform(shader, "u_shadow_matrices[1]"), 1, GL_FALSE, *light->transform[1]);
  glUniformMatrix4fv(ex_uniform(shader, "u_shadow_matrices[2]"), 1, GL_FALSE, *light->transform[2]);
  glUniformMatrix4fv(ex_uniform(shader, "u_shadow_matrices[3]"), 1, GL_FALSE, *light->transform[3]);
  glUniformMatrix4fv(ex_uniform(shader, "u_shadow_matrices[4]"), 1, GL_FALSE, *light->transform[4]);
  glUniformMatrix4fv(ex_uniform(shader, "u_shadow_matrices[5]"), 1, GL_FALSE, *light->transform[5]);

  glUniform1f(ex_uniform(shader, "u_far_plane"), EX_POINT_FAR_PLANE);
  glUniform3fv(ex_uniform(shader, "u_light_pos"), 1, light->position);
}

void ex_render_point_light(ex_point_light_t *light, GLuint shader, const char *prefix)
{
  if (light->is_shadow) {
    glUniform1i(ex_uniform(shader, "u_point_light.is_shadow"), 1);

    glUniform1i(ex_uniform(shader, "u_point_depth"), 7);

    glActiveTexture(GL_TEXTURE7);
    glBindTexture(GL_TEXTURE_CUBE_MAP, light->depth_map);
  } else if (prefix != NULL) {
    char buff[64];
    sprintf(buff, "%s.is_shadow", prefix);
    glUniform1i(ex_uniform(shader, buff), 0);
  }

  if (prefix != NULL) {
    char buff[64];
    sprintf(buff, "%s.far", prefix);
    glUniform1f(ex_uniform(shader,  buff), EX_POINT_FAR_PLANE);
    sprintf(buff, "%s.position", prefix);
    glUniform3fv(ex_uniform(shader, buff), 1, light->position);
    sprintf(buff, "%s.color", prefix);
    glUniform3fv(ex_uniform(shader, buff), 1, light->color);
  } else {
    glUniform1i(ex_uniform(shader,  "u_point_active"), 1);
    glUniform1f(ex_uniform(shader,  "u_point_light.far"), EX_POINT_FAR_PLANE);
    glUniform3fv(ex_uniform(shader, "u_point_light.position"), 1, light->position);
    glUniform3fv(ex_uniform(shader, "u_point_light.color"), 1, light->color);
  }
}

void ex_render_resize(size_t width, size_t height)
{
  /* -- resze framebuffer -- */
  glBindFramebuffer(GL_FRAMEBUFFER, framebuffer.fbo);

  framebuffer.width  = (size_t)((float)display.width  * ex_render_scale);
  framebuffer.height = (size_t)((float)display.height * ex_render_scale);
  if (framebuffer.width  < 16) framebuffer.width  = 16;
  if (framebuffer.height < 16) framebuffer.height = 16;

  glBindTexture(GL_TEXTURE_2D, framebuffer.cbo);
  glTexImage2D(GL_TEXTURE_2D, 0, GL_RGB10_A2, framebuffer.width, framebuffer.height, 0, GL_RGBA, GL_UNSIGNED_BYTE, NULL);

  glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_LINEAR);
  glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_LINEAR);
  glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_R, GL_CLAMP_TO_BORDER);
  glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_S, GL_CLAMP_TO_BORDER);
  glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_T, GL_CLAMP_TO_BORDER);
  GLfloat border[] = {1.0, 1.0, 1.0, 1.0};
  glTexParameterfv(GL_TEXTURE_CUBE_MAP, GL_TEXTURE_BORDER_COLOR, border);
  glBindTexture(GL_TEXTURE_2D, 0);
  glFramebufferTexture2D(GL_FRAMEBUFFER, GL_COLOR_ATTACHMENT0, GL_TEXTURE_2D, framebuffer.cbo, 0);

  // depth buffer -- as a TEXTURE now, so the post pass can sample it (SSAO)
  if (!canvas_depth_tex) glGenTextures(1, &canvas_depth_tex);
  glBindTexture(GL_TEXTURE_2D, canvas_depth_tex);
  glTexImage2D(GL_TEXTURE_2D, 0, GL_DEPTH24_STENCIL8, framebuffer.width, framebuffer.height, 0, GL_DEPTH_STENCIL, GL_UNSIGNED_INT_24_8, NULL);
  glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_NEAREST);
  glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_NEAREST);
  glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_S, GL_CLAMP_TO_EDGE);
  glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_T, GL_CLAMP_TO_EDGE);
  glBindTexture(GL_TEXTURE_2D, 0);
  glFramebufferTexture2D(GL_FRAMEBUFFER, GL_DEPTH_STENCIL_ATTACHMENT, GL_TEXTURE_2D, canvas_depth_tex, 0);

  // test framebuffer
  if (glCheckFramebufferStatus(GL_FRAMEBUFFER) != GL_FRAMEBUFFER_COMPLETE)
    printf("Error! Framebuffer is not complete\n");
  glBindFramebuffer(GL_FRAMEBUFFER, 0);
  /* ----------------- */
}

void ex_render_destroy()
{
  glDeleteRenderbuffers(1, &framebuffer.rbo);
  glDeleteFramebuffers(1, &framebuffer.fbo);
  glDeleteTextures(1, &framebuffer.cbo);

  glDeleteBuffers(1, &screenquad_vbo);
  glDeleteVertexArrays(1, &screenquad_vao);
}