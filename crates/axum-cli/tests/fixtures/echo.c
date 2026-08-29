/* A tool peer written against the documentation, not against axum's code.
 *
 * Two peers built from the same Rust codec cannot disagree with the host, so they cannot show
 * that the protocol is written down anywhere except in the code that speaks it. This one
 * shares nothing: no axum crate, no CBOR library, no Rust. Every byte below was produced from
 * the wire format as documented, which is the only way to find out whether the documentation
 * is enough.
 *
 * Build:  cc -o echo examples/peers/echo.c
 * Check:  it is run by `crates/axum-cli/tests/conformance.rs`, which is the point of it.
 *
 * THE WIRE FORMAT, in full:
 *
 *   A frame is a 4-byte big-endian length, then that many bytes of CBOR.
 *   The CBOR is an envelope: a map of {"version": 0, "body": <message>}.
 *   A message is a map with a "message" key naming its variant, plus that variant's fields.
 *
 *   Peer to host, on connect, once per tool it offers:
 *     {"message":"declare","name":...,"description":...,"parameters":<JSON Schema>}
 *   Host to peer:
 *     {"message":"call","id":...,"name":...,"arguments":<object>}
 *     {"message":"cancel","id":...}
 *   Peer to host, in answer:
 *     {"message":"result","id":...,"output":...,"is_error":<bool>}
 *     {"message":"progress","id":...,"chunk":...}   (optional, any number, before the result)
 *
 *   Every call must be answered, including a call for a tool this peer does not offer.
 *   Closing the peer's stdin ends it.
 *
 * Only the CBOR needed for that is implemented: text strings, byte-counted maps, small
 * integers and booleans. That is the whole surface, which is the useful thing this proves.
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

/* ---- CBOR, enough of it ------------------------------------------------ */

static unsigned char out[1 << 16];
static size_t out_len;

static void put(unsigned char b) { out[out_len++] = b; }

/* A CBOR head: the major type in the top 3 bits, the argument below it. */
static void head(int major, unsigned long long arg) {
    unsigned char m = (unsigned char)(major << 5);
    if (arg < 24) {
        put((unsigned char)(m | arg));
    } else if (arg < 256) {
        put((unsigned char)(m | 24));
        put((unsigned char)arg);
    } else {
        put((unsigned char)(m | 25));
        put((unsigned char)(arg >> 8));
        put((unsigned char)(arg & 0xff));
    }
}

static void text(const char *s) {
    size_t n = strlen(s);
    head(3, n);
    memcpy(out + out_len, s, n);
    out_len += n;
}

static void map(unsigned long long pairs) { head(5, pairs); }
static void whole(unsigned long long n) { head(0, n); }
static void boolean(int b) { put((unsigned char)(b ? 0xf5 : 0xf4)); }

/* ---- Frames ------------------------------------------------------------ */

/* Wrap whatever has been encoded in an envelope and write it with its length. */
static void send_framed(void) {
    unsigned char body[1 << 16];
    size_t body_len = out_len;
    memcpy(body, out, body_len);

    out_len = 0;
    map(2);
    text("version");
    whole(0);
    text("body");
    memcpy(out + out_len, body, body_len);
    out_len += body_len;

    unsigned char len[4];
    len[0] = (unsigned char)(out_len >> 24);
    len[1] = (unsigned char)(out_len >> 16);
    len[2] = (unsigned char)(out_len >> 8);
    len[3] = (unsigned char)(out_len);
    fwrite(len, 1, 4, stdout);
    fwrite(out, 1, out_len, stdout);
    fflush(stdout);
    out_len = 0;
}

static void declare(void) {
    map(4);
    text("message");
    text("declare");
    text("name");
    text("echo");
    text("description");
    text("Repeat what it is given, from a peer that shares no code with axum.");
    text("parameters");
    map(3);
    text("type");
    text("object");
    text("properties");
    map(1);
    text("text");
    map(1);
    text("type");
    text("string");
    text("required");
    head(4, 1); /* array of one */
    text("text");
    send_framed();
}

static void result(const char *id, const char *output, int is_error) {
    map(4);
    text("message");
    text("result");
    text("id");
    text(id);
    text("output");
    text(output);
    text("is_error");
    boolean(is_error);
    send_framed();
}

/* ---- Reading ------------------------------------------------------------ */

/* The decoder only has to find two things in the request: the message kind and the id. Both
 * are text strings, so scanning for them is enough for a peer this size and avoids a full
 * CBOR reader. A peer doing real work would decode properly. */
static int find_text_after(const unsigned char *buf, size_t n, const char *key, char *into,
                           size_t into_len) {
    size_t klen = strlen(key);
    for (size_t i = 0; i + klen + 1 < n; i++) {
        /* A text string head for `klen` bytes, immediately followed by the key. */
        if (buf[i] == (unsigned char)(0x60 | klen) && memcmp(buf + i + 1, key, klen) == 0) {
            size_t at = i + 1 + klen;
            if (at >= n || (buf[at] >> 5) != 3) return 0;
            size_t vlen = buf[at] & 0x1f;
            if (vlen >= 24 || at + 1 + vlen > n || vlen >= into_len) return 0;
            memcpy(into, buf + at + 1, vlen);
            into[vlen] = 0;
            return 1;
        }
    }
    return 0;
}

int main(void) {
    declare();

    for (;;) {
        unsigned char len[4];
        if (fread(len, 1, 4, stdin) != 4) return 0; /* stdin closed: the peer ends */
        size_t n = ((size_t)len[0] << 24) | ((size_t)len[1] << 16) | ((size_t)len[2] << 8) |
                   (size_t)len[3];
        if (n > sizeof out) return 1;
        unsigned char *buf = malloc(n ? n : 1);
        if (!buf || fread(buf, 1, n, stdin) != n) { free(buf); return 0; }

        char kind[32] = {0};
        char id[64] = {0};
        char text_arg[256] = {0};
        find_text_after(buf, n, "message", kind, sizeof kind);
        find_text_after(buf, n, "id", id, sizeof id);
        find_text_after(buf, n, "text", text_arg, sizeof text_arg);
        char name[64] = {0};
        find_text_after(buf, n, "name", name, sizeof name);
        free(buf);

        if (strcmp(kind, "call") == 0) {
            if (strcmp(name, "echo") != 0) {
                /* Answered rather than ignored: silence would leave the host waiting out its
                 * timeout on a mistake it could have been told about at once. */
                result(id, "this peer only offers \"echo\"", 1);
            } else {
                result(id, text_arg[0] ? text_arg : "(nothing to echo)", 0);
            }
        }
        /* A cancel needs no answer: the call it names is already finished by the time this
         * peer could read it, because this peer answers before it reads again. */
    }
}
