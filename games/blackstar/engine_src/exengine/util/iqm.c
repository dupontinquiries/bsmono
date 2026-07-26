#include <math.h>
#include "util/tangents.h"
#include "util/iqm.h"
#include "util/io.h"
#include "util/cache.h"
#include <string.h>

// Look up a texture, falling back to the other common extension. Colour maps are
// stored as .jpg (much smaller) while normal/spec maps stay .png (JPEG artifacts
// wreck normals), but companion names are derived from the diffuse name -- so a
// derived "norm_foo.jpg" must still find the real "norm_foo.png".
static GLuint ex_cache_texture_alt(const char *name)
{
  GLuint id = ex_cache_texture(name);
  if (id) return id;
  size_t n = strlen(name);
  if (n < 5) return 0;
  char alt[512];
  if (n >= sizeof(alt)) return 0;
  strcpy(alt, name);
  if (strcmp(alt + n - 4, ".jpg") == 0)      strcpy(alt + n - 4, ".png");
  else if (strcmp(alt + n - 4, ".png") == 0) strcpy(alt + n - 4, ".jpg");
  else return 0;
  return ex_cache_texture(alt);
}

ex_model_t *ex_iqm_load_model(ex_scene_t *scene, const char *path, uint8_t flags)
{
  // check if its already in the cache
  ex_model_t *m_cache = ex_cache_get_model(path);
  if (m_cache != NULL)
    return m_cache;

  printf("Loading IQM model file %s\n", path);

  // read in the file data
  uint8_t *data = (uint8_t*)ex_io_read(path, "rb", NULL);
  if (data == NULL) {
    printf("Failed to load IQM model file %s\n", path);
    return NULL;
  }

  // the header contents
  ex_iqm_header_t header;

  // check magic string and version
  memcpy(header.magic, data, 16);
  uint *head = (uint *)&data[16];
  header.version = head[0];
  if (strcmp(header.magic, EX_IQM_MAGIC) != 0 || header.version != EX_IQM_VERSION) {
    printf("Loaded IQM model version is not 2.0\nFailed loading %s\n", path);
    free(data);
    return NULL;
  }

  // get the rest of the header weeeeee
  memcpy(&header, data, sizeof(ex_iqm_header_t));

  ex_iqmex_mesh_t *meshes = (ex_iqmex_mesh_t *)&data[header.ofs_meshes];
  char *file_text = header.ofs_text ? (char *)&data[header.ofs_text] : "";

  // set the vertices
  ex_vertex_t *vertices  = malloc(sizeof(ex_vertex_t)*(header.num_vertexes));
  float *position, *uv, *normal, *tangent;
  int has_tangent = 0;
  uint8_t *blend_indexes, *blend_weights, *color;
  ex_iqmvertexarray_t *vas = (ex_iqmvertexarray_t *)&data[header.ofs_vertexarrays];
  for (int i=0; i<header.num_vertexarrays; i++) {
    ex_iqmvertexarray_t va = vas[i];

    switch (va.type) {
      case IQM_POSITION: {
        position = (float *)&data[va.offset];
        for (int x=0; x<header.num_vertexes; x++)
          memcpy(&vertices[x].position, &position[x*va.size], va.size*sizeof(float));
        break;
      }
      case IQM_TEXCOORD: {
        uv = (float *)&data[va.offset];
        for (int x=0; x<header.num_vertexes; x++)
          memcpy(&vertices[x].uv, &uv[x*va.size], va.size*sizeof(float));
        break;
      }
      case IQM_NORMAL: {
        normal = (float *)&data[va.offset];
        for (int x=0; x<header.num_vertexes; x++)
          memcpy(&vertices[x].normal, &normal[x*va.size], va.size*sizeof(float));
        break;
      }
      case IQM_TANGENT: {
        tangent = (float *)&data[va.offset];
        has_tangent = 1;
        for (int x=0; x<header.num_vertexes; x++)
          memcpy(&vertices[x].tangent, &tangent[x*va.size], va.size*sizeof(float));
        break;
      }
      case IQM_BLENDINDEXES: {
        blend_indexes = (uint8_t *)&data[va.offset];
        for (int x=0; x<header.num_vertexes; x++)
          memcpy(&vertices[x].blend_indexes, &blend_indexes[x*va.size], va.size*sizeof(uint8_t));
        break;
      }
      case IQM_BLENDWEIGHTS: {
        blend_weights = (uint8_t *)&data[va.offset];
        for (int x=0; x<header.num_vertexes; x++)
          memcpy(&vertices[x].blend_weights, &blend_weights[x*va.size], va.size*sizeof(uint8_t));
        break;
      }
      case IQM_COLOR: {
        color = (uint8_t *)&data[va.offset];
        for (int x=0; x<header.num_vertexes; x++)
          memcpy(&vertices[x].color, &color[x*va.size], va.size*sizeof(uint8_t));
        break;
      }
    }
  }

  // bones and joints
  ex_bone_t *bones      = NULL;
  ex_frame_t bind_pose  = NULL;
  ex_frame_t pose       = NULL;
  ex_iqmjoint_t *joints  = (ex_iqmjoint_t *)&data[header.ofs_joints];
  if (header.ofs_joints > 0) {
    bones     = malloc(sizeof(ex_bone_t)*header.num_joints);
    bind_pose = malloc(sizeof(ex_pose_t)*header.num_joints);
    pose      = malloc(sizeof(ex_pose_t)*header.num_joints);
    printf("Bones: ");
    for (int i=0; i<header.num_joints; i++) {
      ex_iqmjoint_t *j   = &joints[i];
      strncpy(bones[i].name, &file_text[j->name], 64);
      printf("%s ", bones[i].name);
      bones[i].parent = j->parent;
      memcpy(bones[i].position, j->translate, sizeof(vec3));
      memcpy(bones[i].rotation, j->rotate,    sizeof(quat));
      memcpy(bones[i].scale,    j->scale,     sizeof(vec3));
      memcpy(bind_pose[i].translate,  j->translate, sizeof(vec3));
      memcpy(bind_pose[i].rotate,     j->rotate,    sizeof(quat));
      memcpy(bind_pose[i].scale,      j->scale,     sizeof(vec3));
    }
    printf("\n");
  }

  // anims
  ex_anim_t *anims = NULL;
  ex_iqmex_anim_t *animdata = (ex_iqmex_anim_t *)&data[header.ofs_anims];
  if (header.ofs_anims > 0) {
    anims = malloc(sizeof(ex_anim_t)*header.num_anims);
    for (int i=0; i<header.num_anims; i++) {
      ex_iqmex_anim_t *a    = &animdata[i];

      /* get anim name */
      uint32_t ofs_name = a->name;
      char *name = &file_text[ofs_name]; 
      uint8_t len = strlen(name); 
      anims[i].name   = malloc(sizeof(char) * (len+1));

      strcpy(anims[i].name, name);
      anims[i].name[len+1] = '\0';

      anims[i].first  = a->first_frame;
      anims[i].last   = a->num_frames;
      anims[i].rate   = a->framerate;
      anims[i].loop   = a->flags || (1<<0);
    }  
  }

  // poses
  unsigned short *framedata = NULL;
  ex_frame_t *frames = NULL;
  ex_iqmex_pose_t *posedata = (ex_iqmex_pose_t *)&data[header.ofs_poses];
  if (header.ofs_poses > 0) {
    frames = malloc(sizeof(ex_frame_t)*header.num_frames);
    framedata = (unsigned short *)&data[header.ofs_frames];
    
    for (int i=0; i<header.num_frames; i++) {
      ex_pose_t *frame = malloc(header.num_poses*sizeof(ex_pose_t));
      for (int p=0; p<header.num_poses; p++) {
        ex_iqmex_pose_t *pose = &posedata[p];
        
        float v[10];
        for (int o=0; o<10; o++) {
          float val = pose->channeloffset[o];
          uint mask = (1 << o);
          if ((pose->channelmask & mask) > 0) {
            val += framedata[0] * pose->channelscale[o];
            framedata++;
          }
          v[o] = val;
        }

        memcpy(frame[p].translate, &v[0], sizeof(vec3));
        memcpy(frame[p].rotate,    &v[3], sizeof(quat));
        memcpy(frame[p].scale,     &v[7], sizeof(vec3));
      }
      frames[i] = frame;
    }
  }

  // indices
  GLuint *indices = malloc(sizeof(GLuint)*(header.num_triangles*3));
  memcpy(indices, &data[header.ofs_triangles], (header.num_triangles*3)*sizeof(GLuint));
  int i, a;
  for (i=0; i<header.num_triangles*3; i+=3) {
    a = indices[i+0];
    indices[i+0] = indices[i+2];
    indices[i+2] = a;
  }

  // If the mesh shipped without tangents (most of our procedurally-generated IQMs
  // do), derive them from the UV gradient so normal mapping actually has a valid
  // tangent basis. Without this the TBN is degenerate and only survives because a
  // flat normal map collapses TBN*(0,0,1) back to the geometric normal.
  if (!has_tangent && header.num_vertexes > 0 && header.num_triangles > 0) {
    int nv = header.num_vertexes;
    float *pos = malloc(sizeof(float)*nv*3), *uvp = malloc(sizeof(float)*nv*2);
    float *nrm = malloc(sizeof(float)*nv*3), *tn4 = malloc(sizeof(float)*nv*4);
    if (pos && uvp && nrm && tn4) {
      for (int x=0; x<nv; x++) {
        pos[x*3]=vertices[x].position[0]; pos[x*3+1]=vertices[x].position[1]; pos[x*3+2]=vertices[x].position[2];
        uvp[x*2]=vertices[x].uv[0];       uvp[x*2+1]=vertices[x].uv[1];
        nrm[x*3]=vertices[x].normal[0];   nrm[x*3+1]=vertices[x].normal[1];   nrm[x*3+2]=vertices[x].normal[2];
      }
      ex_compute_tangents(pos, uvp, nrm, tn4, nv, indices, header.num_triangles);
      for (int x=0; x<nv; x++)
        for (int k=0;k<4;k++) vertices[x].tangent[k] = tn4[x*4+k];
    }
    free(pos); free(uvp); free(nrm); free(tn4);
  }

  // create the model
  ex_model_t *model = ex_model_new();
  model->bones       = bones;
  model->anims       = anims;
  model->frames      = frames;
  model->bones_len   = header.num_joints;
  model->anims_len   = header.num_anims;
  model->frames_len  = header.num_frames;
  model->bind_pose   = bind_pose;
  model->pose        = pose;
  model->vertices    = NULL;
  model->octree_data = NULL;

  // calc inverse base pose
  model->inverse_base = NULL;
  model->skeleton     = NULL;
  for (int i=0; i<header.num_joints; i++) {
    if (header.ofs_joints < 1)
      break;

    if (!i) {
      model->inverse_base = malloc(sizeof(mat4x4)*header.num_joints);
      model->skeleton = malloc(sizeof(mat4x4)*header.num_joints);
    }

    ex_bone_t b = model->bones[i];

    mat4x4 mat, inv;
    ex_calc_bone_matrix(mat, b.position, b.rotation, b.scale);
    mat4x4_invert(inv, mat);

    if (b.parent >= 0) {
      mat4x4_mul(model->inverse_base[i], model->inverse_base[b.parent], inv);
    } else {
      mat4x4_dup(model->inverse_base[i], inv);
    }

    if (i >= header.num_joints-1)
      ex_model_update_matrices(model);
  }

  // backup vertices of visible meshes
  vec3 *vis_vertices = malloc(sizeof(vec3)*header.num_triangles*3);
  size_t vis_len = 0;
  
  // add the meshes to the model
  GLuint index_offset = 0;
  for (int i=0; i<header.num_meshes; i++) {
    ex_vertex_t *vert = &vertices[meshes[i].first_vertex];
    GLuint *ind       = &indices[meshes[i].first_triangle*3];

    // negative offset indices
    GLuint offset = 0;
    for (int k=0; k<meshes[i].num_triangles*3; k++) {
      ind[k] -= index_offset;

      if (ind[k] > offset)
        offset = ind[k];
    }
    index_offset += ++offset;

    // get material and texture names
    char *tex_name = &file_text[meshes[i].material];
    char *is_file = strpbrk(tex_name, ".");
    
    // create mesh
    ex_mesh_t *m = ex_mesh_new(vert, meshes[i].num_vertexes, ind, meshes[i].num_triangles*3, 0);

    // store vertices
    if (flags & EX_KEEP_VERTICES) {
      size_t size = meshes[i].num_triangles*3;
      for (int j=0; j<size; j++)
        memcpy(&vis_vertices[vis_len+j], vert[ind[j]].position, sizeof(vec3));
      
      vis_len += size;
    }

    // load textures
    char *tex_types[] = {"spec_", "norm_"};
    if (is_file != NULL) {
      // diffuse
      m->texture = ex_cache_texture(tex_name);

      // don't derive maps-of-maps: if the diffuse is itself a spec_/norm_/emit_
      // texture (some meshes use one directly), skip the aux lookups entirely.
      int is_map = (strncmp(tex_name,"spec_",5)==0 ||
                    strncmp(tex_name,"norm_",5)==0 ||
                    strncmp(tex_name,"emit_",5)==0);
      if (!is_map) {
        // spec (+1 for the null terminator; the old size overflowed by one byte)
        char spec[strlen(tex_name)+strlen(tex_types[0])+1];
        strcpy(spec, tex_types[0]);
        strcpy(&spec[strlen(tex_types[0])], tex_name);
        m->texture_spec = ex_cache_texture_alt(spec);

        // norm
        char norm[strlen(tex_name)+strlen(tex_types[1])+1];
        strcpy(norm, tex_types[1]);
        strcpy(&norm[strlen(tex_types[1])], tex_name);
        m->texture_norm = ex_cache_texture_alt(norm);

        // emission (emit_<tex>): additive self-lit detail; absent -> 0 (no glow)
        char emit[strlen(tex_name)+6];
        strcpy(emit, "emit_");
        strcpy(&emit[5], tex_name);
        m->texture_emission = ex_cache_texture_alt(emit);
      }
    }

    // push mesh into mesh list
    ex_model_add_mesh(model, m);
  }

  // store vertices
  if (flags & EX_KEEP_VERTICES) {
    model->vertices = malloc(vis_len*sizeof(vec3));
    model->num_vertices = vis_len;
    memcpy(model->vertices, vis_vertices, vis_len*sizeof(vec3));
    ex_scene_add_collision(scene, model);
    free(model->vertices);
  }

  // store the path for caching purposes
  strcpy(model->path, path);

  // cleanup data
  printf("Finished loading IQM model %s\n", path);
  free(vertices);
  free(indices);
  free(data);

  // store the model in the cache and return an instance of it
  ex_cache_model(model);
  return ex_cache_get_model(path);
}