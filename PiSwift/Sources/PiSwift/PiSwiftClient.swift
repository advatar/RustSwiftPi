import Foundation
import PiRustFFI

public struct PiSwiftError: Error, CustomStringConvertible, Sendable, Equatable {
    public let message: String

    public init(_ message: String) {
        self.message = message
    }

    public var description: String { message }
}

public struct PiSwiftConfig: Sendable, Equatable {
    public var apiKey: String?
    public var baseURL: String?
    public var model: String
    public var systemPrompt: String?
    public var cwd: URL?

    public init(
        apiKey: String? = nil,
        baseURL: String? = nil,
        model: String = "gpt-4o-mini",
        systemPrompt: String? = nil,
        cwd: URL? = nil
    ) {
        self.apiKey = apiKey
        self.baseURL = baseURL
        self.model = model
        self.systemPrompt = systemPrompt
        self.cwd = cwd
    }
}

public final class PiSwiftClient: Sendable {
    public let config: PiSwiftConfig

    public init(config: PiSwiftConfig) {
        self.config = config
    }

    public func run(prompt: String) async throws -> String {
        try await Task.detached(priority: .userInitiated) { [config] in
            try Self.runBlocking(config: config, prompt: prompt)
        }.value
    }

    public func runTranscriptJSON(prompt: String) async throws -> String {
        try await Task.detached(priority: .userInitiated) { [config] in
            try Self.runTranscriptJSONBlocking(config: config, prompt: prompt)
        }.value
    }

    private static func withOptionalCString<R>(
        _ s: String?,
        _ body: (UnsafePointer<CChar>?) throws -> R
    ) rethrows -> R {
        guard let s else { return try body(nil) }
        return try s.withCString { cs in
            try body(cs)
        }
    }

    private static func runBlocking(config: PiSwiftConfig, prompt: String) throws -> String {
        var outResponse: UnsafeMutablePointer<CChar>?
        var outError: UnsafeMutablePointer<CChar>?

        defer {
            if let outResponse { pi_string_free(outResponse) }
            if let outError { pi_string_free(outError) }
        }

        let cwdPath = config.cwd?.path

        let rc = withOptionalCString(config.apiKey) { apiKey in
            withOptionalCString(config.baseURL) { baseURL in
                config.model.withCString { model in
                    withOptionalCString(config.systemPrompt) { systemPrompt in
                        withOptionalCString(cwdPath) { cwd in
                            prompt.withCString { promptCString in
                                pi_run_prompt(
                                    apiKey,
                                    baseURL,
                                    model,
                                    systemPrompt,
                                    cwd,
                                    promptCString,
                                    &outResponse,
                                    &outError
                                )
                            }
                        }
                    }
                }
            }
        }

        if rc == 0, let outResponse {
            return String(cString: outResponse)
        }

        if let outError {
            throw PiSwiftError(String(cString: outError))
        }

        throw PiSwiftError("PiSwift: unknown error (code \(rc))")
    }

    private static func runTranscriptJSONBlocking(config: PiSwiftConfig, prompt: String) throws -> String {
        var outTranscript: UnsafeMutablePointer<CChar>?
        var outError: UnsafeMutablePointer<CChar>?

        defer {
            if let outTranscript { pi_string_free(outTranscript) }
            if let outError { pi_string_free(outError) }
        }

        let cwdPath = config.cwd?.path

        let rc = withOptionalCString(config.apiKey) { apiKey in
            withOptionalCString(config.baseURL) { baseURL in
                config.model.withCString { model in
                    withOptionalCString(config.systemPrompt) { systemPrompt in
                        withOptionalCString(cwdPath) { cwd in
                            prompt.withCString { promptCString in
                                pi_run_prompt_transcript_json(
                                    apiKey,
                                    baseURL,
                                    model,
                                    systemPrompt,
                                    cwd,
                                    promptCString,
                                    &outTranscript,
                                    &outError
                                )
                            }
                        }
                    }
                }
            }
        }

        if rc == 0, let outTranscript {
            return String(cString: outTranscript)
        }

        if let outError {
            throw PiSwiftError(String(cString: outError))
        }

        throw PiSwiftError("PiSwift: unknown error (code \(rc))")
    }
}
