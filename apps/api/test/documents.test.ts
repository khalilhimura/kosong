/**
 * Document sync: authorization, concurrency, and validation.
 *
 * The load-bearing test is "one user cannot read another's document". §14
 * lists cross-user access as a threat whose control is that the user id comes
 * only from the session and the object key is derived server-side.
 */

import { env } from "cloudflare:test";
import { describe, expect, it } from "vitest";
import { MAX_DOCUMENT_BYTES } from "../src/shared/config";
import { call, fromBase64, okfDocument, signIn, toBase64 } from "./helpers";

/** Writes a document, returning the response. */
async function put(
  accessToken: string,
  text: string,
  ifMatch: string | null = "*",
): Promise<Response> {
  return call("/v1/document", {
    method: "PUT",
    accessToken,
    headers: ifMatch === null ? {} : { "if-match": ifMatch },
    body: { document: toBase64(text) },
  });
}

describe("reading a document", () => {
  it("returns 404 before anything has been synced", async () => {
    const session = await signIn();

    const response = await call("/v1/document", { accessToken: session.access_token });

    expect(response.status).toBe(404);
    expect((await response.json<{ code: string }>()).code).toBe("NOT_FOUND");
  });

  it("returns exactly the bytes that were written", async () => {
    const session = await signIn();
    const text = okfDocument("Round Trip");

    await put(session.access_token, text);
    const response = await call("/v1/document", { accessToken: session.access_token });

    expect(response.status).toBe(200);
    const payload = await response.json<{ document: string; etag: string }>();
    expect(fromBase64(payload.document)).toBe(text);
    expect(payload.etag).toBeTruthy();
  });

  it("preserves unicode and unknown front-matter keys byte for byte", async () => {
    const session = await signIn();
    const text =
      "---\ntype: Page\ntitle: Selamat datang 👋\nunknown_producer_key: kept\n---\n" +
      "# こんにちは\n\nem-dash — and ünïcödé.\n";

    await put(session.access_token, text);
    const response = await call("/v1/document", { accessToken: session.access_token });
    const payload = await response.json<{ document: string }>();

    expect(fromBase64(payload.document)).toBe(text);
  });

  it("requires authentication", async () => {
    const response = await call("/v1/document");
    expect(response.status).toBe(401);
  });

  it("rejects a malformed authorization header", async () => {
    for (const header of ["Bearer", "Basic abc", "abc", "Bearer  "]) {
      const response = await call("/v1/document", {
        headers: { authorization: header },
      });
      expect(response.status).toBe(401);
    }
  });

  it("rejects a tampered access token", async () => {
    const session = await signIn();
    const [header, payload] = session.access_token.split(".");

    const response = await call("/v1/document", {
      accessToken: `${header}.${payload}.wrong-signature`,
    });

    expect(response.status).toBe(401);
  });
});

describe("cross-user authorization", () => {
  it("does not let one user read another's document", async () => {
    const alice = await signIn();
    const bob = await signIn();

    await put(alice.access_token, okfDocument("Alice's page"));

    // Bob is authenticated, just not as Alice.
    const response = await call("/v1/document", { accessToken: bob.access_token });

    expect(response.status).toBe(404);
    const text = await response.text();
    expect(text).not.toContain("Alice");
  });

  it("does not let one user overwrite another's document", async () => {
    const alice = await signIn();
    const bob = await signIn();

    await put(alice.access_token, okfDocument("Alice's page"));
    await put(bob.access_token, okfDocument("Bob's page"));

    const response = await call("/v1/document", { accessToken: alice.access_token });
    const payload = await response.json<{ document: string }>();

    expect(fromBase64(payload.document)).toContain("Alice's page");
  });

  it("keys storage by user, never by anything the client sends", async () => {
    const session = await signIn();
    await put(session.access_token, okfDocument());

    const row = await env.DB.prepare(`SELECT object_key FROM documents WHERE user_id = ?`)
      .bind(session.user.id)
      .first<{ object_key: string }>();

    expect(row!.object_key).toBe(`users/${session.user.id}/document-v1`);
  });
});

describe("optimistic concurrency", () => {
  it("requires an If-Match header", async () => {
    const session = await signIn();

    const response = await put(session.access_token, okfDocument(), null);

    expect(response.status).toBe(400);
    expect(await response.text()).toContain("If-Match");
  });

  it("accepts a write matching the current version", async () => {
    const session = await signIn();
    const first = await put(session.access_token, okfDocument("One"));
    const { etag } = await first.json<{ etag: string }>();

    const second = await put(session.access_token, okfDocument("Two"), etag);

    expect(second.status).toBe(200);
    const updated = await second.json<{ etag: string }>();
    expect(updated.etag).not.toBe(etag);
  });

  it("rejects a write based on a stale version and keeps both intact", async () => {
    const session = await signIn();

    const first = await put(session.access_token, okfDocument("Original"));
    const { etag: staleEtag } = await first.json<{ etag: string }>();

    // Someone else — another device — writes in between.
    await put(session.access_token, okfDocument("From another device"), staleEtag);

    // The first client still holds the old etag.
    const conflicted = await put(
      session.access_token,
      okfDocument("Would clobber"),
      staleEtag,
    );

    expect(conflicted.status).toBe(409);
    const body = await conflicted.json<{ code: string; remote_etag: string }>();
    expect(body.code).toBe("DOCUMENT_CONFLICT");
    expect(body.remote_etag).toBeTruthy();
    expect(body.remote_etag).not.toBe(staleEtag);

    // Crucially, the remote content is untouched.
    const current = await call("/v1/document", { accessToken: session.access_token });
    const payload = await current.json<{ document: string }>();
    expect(fromBase64(payload.document)).toContain("From another device");
    expect(fromBase64(payload.document)).not.toContain("Would clobber");
  });

  it("rejects an etag write when no remote document exists", async () => {
    const session = await signIn();

    const response = await put(session.access_token, okfDocument(), "some-etag");

    expect(response.status).toBe(409);
  });

  it("issues a new version even when the bytes are identical", async () => {
    // Two writes are two versions. A client holding the earlier one should be
    // told it is stale, regardless of whether the content happened to match.
    const session = await signIn();
    const text = okfDocument("Same");

    const first = await put(session.access_token, text);
    const { etag: firstEtag } = await first.json<{ etag: string }>();

    const second = await put(session.access_token, text, firstEtag);
    const { etag: secondEtag } = await second.json<{ etag: string }>();

    expect(secondEtag).not.toBe(firstEtag);
  });
});

describe("validation", () => {
  it("rejects a document that is not in the Open Knowledge Format", async () => {
    const session = await signIn();

    const response = await put(session.access_token, "# Just markdown, no front matter\n");

    expect(response.status).toBe(400);
    expect((await response.json<{ code: string }>()).code).toBe("INVALID_OKF");
  });

  it("rejects front matter with no type", async () => {
    const session = await signIn();

    const response = await put(session.access_token, "---\ntitle: No type\n---\nbody\n");

    expect(response.status).toBe(400);
    const body = await response.json<{ code: string; message: string }>();
    expect(body.code).toBe("INVALID_OKF");
    expect(body.message).toContain("type");
  });

  it("accepts a document carrying only type, as the format requires", async () => {
    const session = await signIn();

    const response = await put(session.access_token, "---\ntype: Page\n---\n# Minimal\n");

    expect(response.status).toBe(200);
  });

  it("accepts an unknown type from another producer", async () => {
    const session = await signIn();

    const response = await put(
      session.access_token,
      "---\ntype: BigQuery Table\ntitle: Orders\n---\n# Schema\n",
    );

    expect(response.status).toBe(200);
  });

  it("rejects a document over the size limit", async () => {
    const session = await signIn();
    const huge = `---\ntype: Page\n---\n${"x".repeat(MAX_DOCUMENT_BYTES + 1024)}`;

    const response = await put(session.access_token, huge);

    expect(response.status).toBe(413);
    expect((await response.json<{ code: string }>()).code).toBe("DOCUMENT_TOO_LARGE");
  });

  it("rejects bytes that are not valid UTF-8", async () => {
    const session = await signIn();
    // 0xFF is never valid in UTF-8.
    const invalid = btoa(String.fromCharCode(0x2d, 0x2d, 0x2d, 0x0a, 0xff, 0xfe));

    const response = await call("/v1/document", {
      method: "PUT",
      accessToken: session.access_token,
      headers: { "if-match": "*" },
      body: { document: invalid },
    });

    expect(response.status).toBe(400);
  });

  it("rejects a body with no document field", async () => {
    const session = await signIn();

    const response = await call("/v1/document", {
      method: "PUT",
      accessToken: session.access_token,
      headers: { "if-match": "*" },
      body: { not_a_document: "x" },
    });

    expect(response.status).toBe(400);
  });

  it("survives a yaml alias bomb without hanging", async () => {
    // Exponential alias expansion is not prevented by the size limit, because
    // the blow-up happens after the bytes are counted.
    const session = await signIn();
    const bomb = [
      "---",
      "type: Page",
      "a: &a [x,x,x,x,x,x,x,x,x]",
      "b: &b [*a,*a,*a,*a,*a,*a,*a,*a,*a]",
      "c: &c [*b,*b,*b,*b,*b,*b,*b,*b,*b]",
      "d: &d [*c,*c,*c,*c,*c,*c,*c,*c,*c]",
      "e: &e [*d,*d,*d,*d,*d,*d,*d,*d,*d]",
      "---",
      "body",
      "",
    ].join("\n");

    const response = await put(session.access_token, bomb);

    // Either outcome is acceptable; hanging or crashing is not.
    expect([200, 400]).toContain(response.status);
  });
});
