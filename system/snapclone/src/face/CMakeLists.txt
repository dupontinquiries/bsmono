# ─── snapclone_face ────────────────────────────────────────────────────────
# Face library wrapper — RAII C++ shim around the Rust snapclone-face crate.
#
# When SNAPCLONE_RUST_FACE=OFF (default):
#   The wrapper still compiles but all methods return NotCompiled. Callers
#   can use the Library class unconditionally; behavior degrades gracefully
#   to "no face library, send full image."
#
# When SNAPCLONE_RUST_FACE=ON:
#   Links against the libsnapclone_face.a static library built by the
#   snapclone_rust_face_build custom target (root CMakeLists). The C
#   header is hand-curated at include/face/snapclone_face_ffi.h.
# ───────────────────────────────────────────────────────────────────────────

set(FACE_SOURCES FaceLibrary.cpp)
set(FACE_HEADERS
    ${PROJECT_SOURCE_DIR}/include/face/FaceLibrary.hpp
    ${PROJECT_SOURCE_DIR}/include/face/snapclone_face_ffi.h
)

add_library(snapclone_face STATIC
    ${FACE_SOURCES}
    ${FACE_HEADERS}
)

target_include_directories(snapclone_face
    PUBLIC
        ${PROJECT_SOURCE_DIR}/include
)

target_link_libraries(snapclone_face
    PUBLIC
        snapclone_warnings
        Qt6::Core
)

# Link the Rust static library when the backend is active.
if(SNAPCLONE_RUST_FACE)
    target_link_libraries(snapclone_face PUBLIC snapclone_rust_face_lib)
    # The Rust staticlib brings in tract-onnx + std dependencies that
    # transitively require these system libs.
    target_link_libraries(snapclone_face PUBLIC dl pthread)
endif()

set_target_properties(snapclone_face PROPERTIES AUTOMOC ON)
