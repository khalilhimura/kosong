/**
 * Document sync logic: read, write, and conflict detection.
 *
 * The concurrency rule from §9.3: an `If-Match` mismatch returns 409 with the
 * remote metadata. It never overwrites and never merges. v1 has no way to
 * reconcile two versions of prose, so the honest move is to hand both back to
 * the user and let them decide.
 */

import { MAX_DOCUMENT_BYTES, type Env } from "../../shared/config";
import { generateId } from "../../shared/crypto";
import { ApiError } from "../../shared/errors";
import { recordSecurityEvent, type Logger } from "../../shared/logging";
import { DocumentRepository, type DocumentMetadata } from "./document.repository";
import { validateOkf } from "./okf";

export interface DocumentPayload {
  etag: string;
  updated_at: string;
  /** Raw OKF bytes, base64 encoded. */
  document: string;
}

export class DocumentService {
  private readonly repository: DocumentRepository;

  constructor(
    private readonly env: Env,
    private readonly logger: Logger,
  ) {
    this.repository = new DocumentRepository(env);
  }

  /** Reads the user's document. 404 when they have never synced one. */
  async read(userId: string): Promise<DocumentPayload> {
    const metadata = await this.repository.findMetadata(userId);
    if (!metadata) {
      throw ApiError.notFound("You have not synced a document yet.");
    }

    const bytes = await this.repository.readBytes(userId);
    if (!bytes) {
      // Metadata without an object means a write was interrupted between the
      // two stores. Reported as absent rather than as a server error: from the
      // user's point of view there is nothing to pull.
      this.logger.error({
        event: "document_object_missing",
        outcome: "failure",
        user_id: userId,
      });
      throw ApiError.notFound("You have not synced a document yet.");
    }

    this.logger.info({ event: "document_read", outcome: "success", user_id: userId });
    await recordSecurityEvent(this.env, this.logger, {
      type: "document_read",
      userId,
    });

    return {
      etag: metadata.etag,
      updated_at: metadata.updated_at,
      document: encodeBase64(new Uint8Array(bytes)),
    };
  }

  /**
   * Writes the user's document, subject to optimistic concurrency.
   *
   * `ifMatch` semantics:
   * - `"*"` — write regardless. The client is claiming a first sync or an
   *   intentional overwrite.
   * - an etag — write only if the remote is still at that version.
   * - absent — rejected. Silently defaulting to overwrite is how data is lost.
   */
  async write(input: {
    userId: string;
    documentBase64: string;
    ifMatch: string | null;
  }): Promise<{ etag: string; updated_at: string }> {
    if (!input.ifMatch) {
      throw ApiError.invalidInput(
        "An If-Match header is required, so a sync cannot overwrite work by accident.",
      );
    }

    const bytes = this.decodeAndValidate(input.documentBase64);
    const existing = await this.repository.findMetadata(input.userId);

    if (input.ifMatch !== "*") {
      if (!existing) {
        throw ApiError.conflict({
          reason: "no remote document exists yet",
          remote_etag: null,
        });
      }
      if (existing.etag !== input.ifMatch) {
        this.logger.warn({
          event: "document_conflict",
          outcome: "failure",
          user_id: input.userId,
        });
        await recordSecurityEvent(this.env, this.logger, {
          type: "document_conflict",
          userId: input.userId,
        });
        throw ApiError.conflict({
          remote_etag: existing.etag,
          remote_updated_at: existing.updated_at,
          remote_byte_size: existing.byte_size,
        });
      }
    }

    // A fresh opaque token per write. Deliberately not a content hash: two
    // writes of identical bytes are still two distinct versions, and a client
    // holding the older one should be told so.
    const written: DocumentMetadata = await this.repository.write({
      userId: input.userId,
      bytes: bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength) as ArrayBuffer,
      etag: generateId(),
    });

    this.logger.info({
      event: "document_written",
      outcome: "success",
      user_id: input.userId,
    });
    await recordSecurityEvent(this.env, this.logger, {
      type: "document_written",
      userId: input.userId,
      metadata: { byte_size: written.byte_size },
    });

    return { etag: written.etag, updated_at: written.updated_at };
  }

  async delete(userId: string): Promise<void> {
    await this.repository.delete(userId);
  }

  /**
   * Decodes base64, then enforces size, UTF-8, and OKF conformance.
   *
   * Order matters. Size is checked on the encoded string first so a huge
   * payload is rejected before anything expensive touches it.
   */
  private decodeAndValidate(documentBase64: string): Uint8Array {
    // base64 is 4 characters per 3 bytes, so this bounds the decode cheaply.
    if (documentBase64.length > Math.ceil((MAX_DOCUMENT_BYTES * 4) / 3) + 8) {
      throw ApiError.tooLarge(MAX_DOCUMENT_BYTES);
    }

    let bytes: Uint8Array;
    try {
      bytes = decodeBase64(documentBase64);
    } catch {
      throw ApiError.invalidInput("The document was not valid base64.");
    }

    if (bytes.byteLength > MAX_DOCUMENT_BYTES) {
      throw ApiError.tooLarge(MAX_DOCUMENT_BYTES);
    }

    let text: string;
    try {
      // `fatal` rejects invalid UTF-8 rather than substituting replacement
      // characters, which would corrupt the document silently.
      text = new TextDecoder("utf-8", { fatal: true, ignoreBOM: false }).decode(bytes);
    } catch {
      throw ApiError.invalidInput("The document must be valid UTF-8 text.");
    }

    const result = validateOkf(text);
    if (!result.valid) {
      throw ApiError.invalidOkf(
        `That document is not in the Open Knowledge Format: ${result.reason}.`,
      );
    }

    return bytes;
  }
}

function encodeBase64(bytes: Uint8Array): string {
  let binary = "";
  // Chunked: spreading a megabyte into String.fromCharCode at once overflows
  // the argument limit.
  const chunkSize = 0x8000;
  for (let i = 0; i < bytes.length; i += chunkSize) {
    binary += String.fromCharCode(...bytes.subarray(i, i + chunkSize));
  }
  return btoa(binary);
}

function decodeBase64(text: string): Uint8Array {
  const binary = atob(text);
  return Uint8Array.from(binary, (char) => char.charCodeAt(0));
}
