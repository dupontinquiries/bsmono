#include "core/GroupMediaSender.hpp"
#include "mediashare/snapclone_mediashare_ffi.h"
#include <QCoreApplication>
#include <vector>
#include <cstdio>
#include <cstdint>
#include <cstring>
using namespace SnapClone::Core;
static int fails=0;
#define CHECK(c,m) do{ if(!(c)){ std::printf("FAIL: %s\n",m); ++fails;} else std::printf("ok:   %s\n",m);}while(0)

static void directFfiRoundTrip() {
    uint8_t ck[32]; sc_media_content_key_new(ck);
    const char* msg = "media-share e2e: seal -> chunk -> reassemble -> open, across several chunks";
    size_t n = std::strlen(msg);
    std::vector<uint8_t> sealed(n+28); size_t slen=0;
    int rc = sc_media_seal(ck,(const uint8_t*)msg,n,sealed.data(),sealed.size(),&slen);
    CHECK(rc==0 && slen==n+28, "FFI seal ok, size == len+28");
    sealed.resize(slen);
    ChunkStore* st = sc_chunk_store_new();
    size_t need=0;
    rc = sc_chunk_store_add_blob(st, sealed.data(), sealed.size(), 16, nullptr, 0, &need);
    CHECK(rc==-2 && need>0, "add_blob sizing → required manifest len");
    std::vector<uint8_t> manifest(need); size_t mlen=0;
    rc = sc_chunk_store_add_blob(st, sealed.data(), sealed.size(), 16, manifest.data(), manifest.size(), &mlen);
    CHECK(rc==0, "add_blob writes manifest"); manifest.resize(mlen);
    std::vector<uint8_t> recov(sealed.size()); size_t rlen=0;
    rc = sc_chunk_store_reassemble(st, manifest.data(), manifest.size(), recov.data(), recov.size(), &rlen);
    CHECK(rc==0 && rlen==sealed.size() && std::memcmp(recov.data(),sealed.data(),slen)==0,
          "reassembled ciphertext == sealed");
    std::vector<uint8_t> pt(recov.size()); size_t plen=0;
    rc = sc_media_open(ck, recov.data(), rlen, pt.data(), pt.size(), &plen);
    CHECK(rc==0 && plen==n && std::memcmp(pt.data(),msg,n)==0, "open → original plaintext");
    sc_chunk_store_free(st);
}

static void senderPrepare() {
    GroupMediaSender sender(16);
    CHECK(sender.isAvailable(), "GroupMediaSender::isAvailable (mediashare built)");
    GroupMediaSender::Prepared got; bool fired=false, failed=false;
    QObject::connect(&sender,&GroupMediaSender::prepared,[&](const GroupMediaSender::Prepared&p){got=p;fired=true;});
    QObject::connect(&sender,&GroupMediaSender::prepareFailed,[&](const QString&){failed=true;});
    QByteArray media("some raw media payload bytes sealed and chunked into many pieces here!!", 69);
    sender.prepare(media, false);
    CHECK(fired && !failed, "prepare() emitted prepared()");
    CHECK(got.ok, "Prepared.ok");
    CHECK(got.originalBytes==media.size() && got.strippedBytes==media.size(), "sizes: no strip for non-image");
    CHECK(got.sealedBytes==media.size()+28, "sealedBytes == stripped + 28");
    CHECK(!got.manifest.isEmpty() && !got.keyShareEnvelope.isEmpty(), "manifest + keyshare produced");
    CHECK(got.recipientCount==0, "0 recipients");
    // serve a chunk: take the content key, can't enumerate ids here, but verify
    // lastContentKey is 32 bytes
    CHECK(sender.lastContentKey().size()==32, "lastContentKey is 32 bytes");
}

int main(int argc, char** argv){
    QCoreApplication app(argc, argv);
    directFfiRoundTrip();
    senderPrepare();
    std::printf(fails? "\n%d FAILED\n":"\nALL CHECKS PASSED\n", fails);
    return fails?1:0;
}
