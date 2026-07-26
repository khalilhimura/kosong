/**
 * Storage for the one document a user owns.
 *
 * Split across two systems: D1 holds metadata and the concurrency token, R2
 * holds the bytes. D1 has a 1 MB row limit and is not the right home for
 * content, and §10 states outright that document content never appears there.
 *
 * The object key is always derived from the authenticated user id by
 * {@link documentKey}. No caller passes a key in, which is what stops one
 * user's requests from addressing another's storage.
 */

import { documentKey, type Env } from "../../shared/config";

export interface DocumentMetadata {
  user_id: string;
  object_key: string;
  etag: string;
  byte_size: number;
  created_at: string;
  updated_at: string;
}

export class DocumentRepository {
  constructor(private readonly env: Env) {}

  async findMetadata(userId: string): Promise<DocumentMetadata | null> {
    return this.env.DB.prepare(
      `SELECT user_id, object_key, etag, byte_size, created_at, updated_at
         FROM documents
        WHERE user_id = ? AND deleted_at IS NULL`,
    )
      .bind(userId)
      .first<DocumentMetadata>();
  }

  async readBytes(userId: string): Promise<ArrayBuffer | null> {
    const object = await this.env.DOCUMENTS.get(documentKey(userId));
    return object ? object.arrayBuffer() : null;
  }

  /**
   * Writes bytes and records the new version.
   *
   * R2 first, then D1. If the D1 write fails, the object is newer than its
   * metadata, and the next successful write corrects it. The reverse order
   * would let metadata advertise a version whose bytes were never stored,
   * which reads as silent data loss.
   */
  async write(input: {
    userId: string;
    bytes: ArrayBuffer;
    etag: string;
  }): Promise<DocumentMetadata> {
    const key = documentKey(input.userId);
    const now = new Date().toISOString();

    await this.env.DOCUMENTS.put(key, input.bytes, {
      httpMetadata: { contentType: "text/markdown; charset=utf-8" },
    });

    await this.env.DB.prepare(
      `INSERT INTO documents (user_id, object_key, etag, byte_size, created_at, updated_at)
       VALUES (?, ?, ?, ?, ?, ?)
       ON CONFLICT(user_id) DO UPDATE SET
         etag = excluded.etag,
         byte_size = excluded.byte_size,
         updated_at = excluded.updated_at,
         deleted_at = NULL`,
    )
      .bind(input.userId, key, input.etag, input.bytes.byteLength, now, now)
      .run();

    const written = await this.findMetadata(input.userId);
    if (!written) {
      throw new Error("document metadata missing immediately after write");
    }
    return written;
  }

  async delete(userId: string): Promise<void> {
    await this.env.DOCUMENTS.delete(documentKey(userId));
    await this.env.DB.prepare(`DELETE FROM documents WHERE user_id = ?`).bind(userId).run();
  }
}
