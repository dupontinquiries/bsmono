#START VS
#version 330 core

layout (location = 0) in vec2 in_position;
layout (location = 1) in vec2 in_uv;

out vec2 uv;

void main() 
{
  gl_Position = vec4(in_position.x, in_position.y, 0.0f, 1.0f);
  uv = in_uv;
}
#END VS


#START FS
#version 330 core

in vec2 uv;

out vec4 color;

uniform sampler2D u_texture;

vec3 aces_tonemap(vec3 x) {
  float a = 2.51;
  float b = 0.03;
  float c = 2.43;
  float d = 0.59;
  float e = 0.14;
  return clamp((x*(a*x+b))/(x*(c*x+d)+e), 0.0, 1.0);
}

const vec3 u_white_point = vec3(1.6, 1.6, 1.6);

void main()
{
  vec3 tex_color = textureLod(u_texture, uv, 0.0).rgb;
  // Exponential exposure. The NEGATIVE sign is essential: 1 - exp(-x) maps
  // [0,inf) -> [0,1). Without it, any value above ~0.1 goes negative and the
  // ACES pass below flips it past 1.0 -> pure white (the "everything is white"
  // bug that showed up once scene brightness rose).
  tex_color = vec3(1.0) - exp(-tex_color / u_white_point);
  color = vec4(aces_tonemap(tex_color), 1.0);
}
#END FS