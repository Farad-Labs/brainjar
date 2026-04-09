// Protocol.swift — Atlas Swift storage protocol and implementations.
//
// Defines the StorageBackend protocol and provides a ClickHouse-backed
// implementation used by the iOS/macOS Atlas edge client.

import Foundation

// MARK: - Type aliases

/// Unique identifier for a stored event row.
typealias RowID = UUID

/// Callback invoked when a write operation completes.
typealias WriteCompletion = (Result<[RowID], AtlasStorageError>) -> Void

// MARK: - Errors

/// Errors surfaced by the Atlas storage layer.
enum AtlasStorageError: Error, LocalizedError {
    case connectionFailed(String)
    case writeFailed(String)
    case serialisationFailed(String)
    case bufferOverflow(Int)

    var errorDescription: String? {
        switch self {
        case .connectionFailed(let msg):  return "Connection failed: \(msg)"
        case .writeFailed(let msg):       return "Write failed: \(msg)"
        case .serialisationFailed(let m): return "Serialisation failed: \(m)"
        case .bufferOverflow(let cap):    return "Buffer overflow at capacity \(cap)"
        }
    }
}

// MARK: - TransformResult

/// Mirror of the transform engine's output, used as the storage write unit.
struct TransformResult: Codable {
    let eventId: String
    let source: String
    let transformed: [String: AnyCodable]
    let rulesApplied: [String]
    let latencyMs: Int64
}

// MARK: - StorageBackend protocol

/// Contract for Atlas storage backends.
///
/// WHY: the protocol lets us swap ClickHouse for SQLite in unit tests and
/// on-device caching scenarios without touching the pipeline orchestrator.
protocol StorageBackend: AnyObject {
    /// Human-readable backend identifier used in metrics.
    var backendName: String { get }

    /// Write a batch of results asynchronously.
    func write(_ rows: [TransformResult], completion: @escaping WriteCompletion)

    /// Flush any in-memory buffers to durable storage.
    func flush() async throws

    /// Close connections and release resources.
    func close() async
}

// MARK: - ClickHouseBackend

/// ClickHouse storage backend for the Atlas pipeline.
final class ClickHouseBackend: StorageBackend {
    let backendName = "clickhouse"

    private let endpoint: URL
    private let database: String
    private let session: URLSession

    // NOTE: we keep a reference to the URLSession so it can be injected in
    // tests with a custom URLProtocol mock.
    init(endpoint: URL, database: String, session: URLSession = .shared) {
        self.endpoint = endpoint
        self.database = database
        self.session = session
    }

    func write(_ rows: [TransformResult], completion: @escaping WriteCompletion) {
        guard !rows.isEmpty else {
            completion(.success([]))
            return
        }

        guard let body = serialise(rows) else {
            completion(.failure(.serialisationFailed("JSON encoding failed")))
            return
        }

        var request = URLRequest(url: endpoint.appendingPathComponent("/write"))
        request.httpMethod = "POST"
        request.httpBody = body
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")

        session.dataTask(with: request) { _, response, error in
            if let error = error {
                completion(.failure(.connectionFailed(error.localizedDescription)))
                return
            }
            guard let http = response as? HTTPURLResponse, http.statusCode == 200 else {
                completion(.failure(.writeFailed("unexpected HTTP status")))
                return
            }
            let ids = rows.map { _ in UUID() }
            completion(.success(ids))
        }.resume()
    }

    func flush() async throws {
        // ClickHouse HTTP interface is synchronous per-request; no-op here.
    }

    func close() async {
        session.invalidateAndCancel()
    }

    // MARK: Private

    private func serialise(_ rows: [TransformResult]) -> Data? {
        try? JSONEncoder().encode(rows)
    }
}

// MARK: - BufferedBackend extension

extension StorageBackend {
    /// Convenience: write a single result, calling the callback on main queue.
    func writeOne(_ row: TransformResult, completion: @escaping WriteCompletion) {
        write([row]) { result in
            DispatchQueue.main.async { completion(result) }
        }
    }
}
