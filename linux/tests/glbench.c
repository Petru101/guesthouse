// SPDX-License-Identifier: GPL-2.0-or-later
/* Desktop OpenGL 4.6 with no window, the same binary on the host and in the guest: exact pixels and speed.
 * Image: integer hash per pixel from a fragment shader, a texture sampled with nearest, depth test and blending.
 * Usage: glbench [out.raw]. Env: SURFACELESS=1 picks Mesa's platform. Build: gcc -O2 -o glbench glbench.c -lEGL -lOpenGL -lm */
#define GL_GLEXT_PROTOTYPES
#include <EGL/egl.h>
#include <EGL/eglext.h>
#include <GL/gl.h>
#include <GL/glext.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

enum { W = 1920, H = 1080 };

static double now_ms(void) { struct timespec t; clock_gettime(CLOCK_MONOTONIC, &t); return t.tv_sec * 1e3 + t.tv_nsec / 1e6; }

static int cmp(const void *a, const void *b) { double x = *(double *)a, y = *(double *)b; return x < y ? -1 : x > y; }

static GLuint program(const char *vs, const char *fs)
{
    GLuint v = glCreateShader(GL_VERTEX_SHADER), f = glCreateShader(GL_FRAGMENT_SHADER), p = glCreateProgram();
    GLint ok;
    char log[1024];
    glShaderSource(v, 1, &vs, NULL); glCompileShader(v);
    glShaderSource(f, 1, &fs, NULL); glCompileShader(f);
    glGetShaderiv(f, GL_COMPILE_STATUS, &ok);
    if (!ok) { glGetShaderInfoLog(f, sizeof(log), NULL, log); printf("shader: %s\n", log); exit(1); }
    glAttachShader(p, v); glAttachShader(p, f); glLinkProgram(p);
    return p;
}

static const char *quad_vs = "#version 460\nvoid main(){vec2 p=vec2((gl_VertexID<<1)&2,gl_VertexID&2);gl_Position=vec4(p*2.-1.,0.,1.);}";
static const char *small_vs = "#version 460\nlayout(location=0) uniform vec2 o;void main(){vec2 p=vec2((gl_VertexID<<1)&2,gl_VertexID&2);gl_Position=vec4(o+p*0.002,0.,1.);}";
static const char *hash_fs = "#version 460\nout vec4 c;void main(){uvec2 q=uvec2(gl_FragCoord.xy);uint h=(q.x*73856093u)^(q.y*19349663u);h^=h>>13;h*=2246822519u;h^=h>>16;c=vec4(uvec4(h,h>>8,h>>16,255u)&255u)/255.;}";
static const char *alu_fs = "#version 460\nlayout(location=1) uniform int n;out vec4 c;void main(){float a=gl_FragCoord.x*1e-3,b=gl_FragCoord.y*1e-3;for(int i=0;i<n;i++){a=a*b+0.5;b=b*a+0.25;a=fract(a);b=fract(b);}c=vec4(a,b,a*b,1.);}";
static const char *tex_fs = "#version 460\nlayout(binding=0) uniform sampler2D t;out vec4 c;void main(){c=texelFetch(t,ivec2(gl_FragCoord.xy)/2,0)*vec4(.5,.75,1.,.5);}";
static const char *flat_fs = "#version 460\nout vec4 c;void main(){c=vec4(1.,.5,.25,1.);}";

int main(int argc, char **argv)
{
    PFNEGLQUERYDEVICESEXTPROC qd = (void *)eglGetProcAddress("eglQueryDevicesEXT");
    PFNEGLGETPLATFORMDISPLAYEXTPROC gd = (void *)eglGetProcAddress("eglGetPlatformDisplayEXT");
    EGLDeviceEXT devs[8];
    EGLint n = 0, maj, min, count;
    EGLDisplay d = EGL_NO_DISPLAY;
    if (getenv("SURFACELESS")) {
        d = gd(EGL_PLATFORM_SURFACELESS_MESA, EGL_DEFAULT_DISPLAY, NULL);
        eglInitialize(d, &maj, &min);
    } else {
        qd(8, devs, &n);
        for (int i = 0; i < n && d == EGL_NO_DISPLAY; i++) {
            EGLDisplay t = gd(EGL_PLATFORM_DEVICE_EXT, devs[i], NULL);
            if (eglInitialize(t, &maj, &min) && strstr(eglQueryString(t, EGL_VENDOR), "NVIDIA")) d = t;
        }
    }
    eglBindAPI(EGL_OPENGL_API);
    EGLint ca[] = { EGL_SURFACE_TYPE, EGL_PBUFFER_BIT, EGL_RENDERABLE_TYPE, EGL_OPENGL_BIT, EGL_NONE };
    EGLConfig c;
    eglChooseConfig(d, ca, &c, 1, &count);
    EGLint xa[] = { EGL_CONTEXT_MAJOR_VERSION, 4, EGL_CONTEXT_MINOR_VERSION, 6, EGL_CONTEXT_OPENGL_PROFILE_MASK, EGL_CONTEXT_OPENGL_CORE_PROFILE_BIT, EGL_NONE };
    EGLContext x = eglCreateContext(d, c, EGL_NO_CONTEXT, xa);
    EGLint pa[] = { EGL_WIDTH, 16, EGL_HEIGHT, 16, EGL_NONE };
    EGLSurface s = eglCreatePbufferSurface(d, c, pa);
    if (!eglMakeCurrent(d, s, s, x)) { printf("make current failed %x\n", eglGetError()); return 1; }
    printf("renderer %s\nversion %s\n", glGetString(GL_RENDERER), glGetString(GL_VERSION));

    GLuint fbo, rb[2], vao, tex, q;
    glGenVertexArrays(1, &vao); glBindVertexArray(vao);
    glGenFramebuffers(1, &fbo); glBindFramebuffer(GL_FRAMEBUFFER, fbo);
    glGenRenderbuffers(2, rb);
    glBindRenderbuffer(GL_RENDERBUFFER, rb[0]); glRenderbufferStorage(GL_RENDERBUFFER, GL_RGBA8, W, H);
    glBindRenderbuffer(GL_RENDERBUFFER, rb[1]); glRenderbufferStorage(GL_RENDERBUFFER, GL_DEPTH24_STENCIL8, W, H);
    glFramebufferRenderbuffer(GL_FRAMEBUFFER, GL_COLOR_ATTACHMENT0, GL_RENDERBUFFER, rb[0]);
    glFramebufferRenderbuffer(GL_FRAMEBUFFER, GL_DEPTH_STENCIL_ATTACHMENT, GL_RENDERBUFFER, rb[1]);
    glViewport(0, 0, W, H);
    GLuint hash = program(quad_vs, hash_fs), alu = program(quad_vs, alu_fs), texp = program(quad_vs, tex_fs), flat = program(small_vs, flat_fs);

    /* the picture */
    static unsigned char texels[(W / 2) * (H / 2) * 4];
    for (size_t i = 0; i < sizeof(texels); i++) texels[i] = (i * 2654435761u) >> 24;
    glGenTextures(1, &tex); glBindTexture(GL_TEXTURE_2D, tex);
    glTexStorage2D(GL_TEXTURE_2D, 1, GL_RGBA8, W / 2, H / 2);
    glTexSubImage2D(GL_TEXTURE_2D, 0, 0, 0, W / 2, H / 2, GL_RGBA, GL_UNSIGNED_BYTE, texels);
    glClearColor(1, 0, 0, 1); glClear(GL_COLOR_BUFFER_BIT | GL_DEPTH_BUFFER_BIT);
    glUseProgram(hash); glDrawArrays(GL_TRIANGLES, 0, 3);
    glEnable(GL_BLEND); glBlendFunc(GL_SRC_ALPHA, GL_ONE_MINUS_SRC_ALPHA);
    glEnable(GL_SCISSOR_TEST); glScissor(200, 100, 900, 700);
    glUseProgram(texp); glDrawArrays(GL_TRIANGLES, 0, 3);
    glDisable(GL_SCISSOR_TEST); glDisable(GL_BLEND);
    static unsigned char px[W * H * 4];
    glReadPixels(0, 0, W, H, GL_RGBA, GL_UNSIGNED_BYTE, px);
    unsigned long sum = 0;
    for (size_t i = 0; i < sizeof(px); i++) sum = sum * 31 + px[i];
    printf("image hash %016lx, glGetError %x\n", sum, glGetError());
    if (argc > 1) { FILE *o = fopen(argv[1], "wb"); fwrite(px, 1, sizeof(px), o); fclose(o); }

    /* speed */
    double t0;
    glGenQueries(1, &q);
    glUseProgram(alu); glUniform1i(1, 4000);
    for (int i = 0; i < 3; i++) glDrawArrays(GL_TRIANGLES, 0, 3);
    glFinish();
    t0 = now_ms();
    int runs = 20;
    glBeginQuery(GL_TIME_ELAPSED, q);
    for (int i = 0; i < runs; i++) glDrawArrays(GL_TRIANGLES, 0, 3);
    glEndQuery(GL_TIME_ELAPSED);
    glFinish();
    GLuint64 ns = 0;
    glGetQueryObjectui64v(q, GL_QUERY_RESULT, &ns);
    printf("alu: %.3f ms a pass on the GPU, %.3f by the wall\n", ns / 1e6 / runs, (now_ms() - t0) / runs);

    glUseProgram(hash);
    glFinish();
    t0 = now_ms();
    glBeginQuery(GL_TIME_ELAPSED, q);
    for (int i = 0; i < 1000; i++) glDrawArrays(GL_TRIANGLES, 0, 3);
    glEndQuery(GL_TIME_ELAPSED);
    glFinish();
    glGetQueryObjectui64v(q, GL_QUERY_RESULT, &ns);
    printf("fill: %.1f us a full-screen pass on the GPU, %.1f by the wall\n", ns / 1e3 / 1000, (now_ms() - t0) * 1e3 / 1000);

    glUseProgram(flat);
    enum { DRAWS = 200000 };
    glFinish();
    t0 = now_ms();
    for (int i = 0; i < DRAWS; i++) {
        glUniform2f(0, (i % 1000) * 0.002f - 1.f, (i / 1000 % 1000) * 0.002f - 1.f);
        glDrawArrays(GL_TRIANGLES, 0, 3);
    }
    double issue = now_ms() - t0;
    glFinish();
    printf("draws: %d with a uniform change, %.3f us each to issue, %.3f us each with finish\n", DRAWS, issue * 1e3 / DRAWS, (now_ms() - t0) * 1e3 / DRAWS);

    enum { RT = 2000 };
    static double r[RT];
    for (int i = 0; i < RT; i++) {
        double a = now_ms();
        glClear(GL_DEPTH_BUFFER_BIT);
        glFinish();
        r[i] = (now_ms() - a) * 1e3;
    }
    qsort(r, RT, sizeof(double), cmp);
    printf("glFinish round trip: median %.1f us, p99 %.1f us\n", r[RT / 2], r[RT * 99 / 100]);

    t0 = now_ms();
    for (int i = 0; i < 50; i++) glReadPixels(0, 0, W, H, GL_RGBA, GL_UNSIGNED_BYTE, px);
    printf("readback: %.2f GB/s\n", 50.0 * sizeof(px) / (now_ms() - t0) / 1e6);
    t0 = now_ms();
    for (int i = 0; i < 200; i++) glTexSubImage2D(GL_TEXTURE_2D, 0, 0, 0, W / 2, H / 2, GL_RGBA, GL_UNSIGNED_BYTE, texels);
    glFinish();
    printf("texture upload: %.2f GB/s\n", 200.0 * sizeof(texels) / (now_ms() - t0) / 1e6);
    return 0;
}
