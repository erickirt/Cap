import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const serverEnvMock = vi.hoisted(() =>
	vi.fn(() => ({
		ANTHROPIC_API_KEY: "anthropic-key",
		OPENAI_API_KEY: undefined,
		GROQ_API_KEY: undefined,
		SUPERMEMORY_API_KEY: undefined,
		SUPERMEMORY_KNOWLEDGE_TAG: undefined,
	})),
);
const searchSupermemoryMock = vi.hoisted(() => vi.fn());
const getGroqClientMock = vi.hoisted(() => vi.fn());

vi.mock("server-only", () => ({}));

vi.mock("@cap/env", () => ({
	serverEnv: serverEnvMock,
}));

vi.mock("@/lib/groq-client", () => ({
	GROQ_MODEL: "groq-model",
	getGroqClient: getGroqClientMock,
}));

vi.mock("@/lib/messenger/supermemory", () => ({
	getKnowledgeTag: vi.fn(() => "knowledge"),
	searchSupermemory: searchSupermemoryMock,
}));

const readFetchBody = (call: unknown[]) => {
	const init = call[1] as { body?: unknown };
	if (typeof init.body !== "string") {
		throw new Error("Expected fetch body");
	}
	return JSON.parse(init.body) as {
		model?: string;
		max_tokens?: number;
		tools?: Array<{
			name?: string;
			input_schema?: {
				properties?: Record<string, unknown>;
			};
		}>;
		messages?: Array<{
			role?: string;
			content?: unknown;
		}>;
	};
};

describe("generateMessengerAgentReply", () => {
	beforeEach(() => {
		vi.clearAllMocks();
		searchSupermemoryMock.mockResolvedValue([]);
		getGroqClientMock.mockReturnValue(null);
	});

	afterEach(() => {
		vi.unstubAllGlobals();
	});

	it("uses Sonnet 5 and executes the constrained support email tool", async () => {
		const fetchMock = vi
			.fn()
			.mockResolvedValueOnce(
				new Response(
					JSON.stringify({
						content: [
							{
								type: "tool_use",
								id: "tool-1",
								name: "send_support_email",
								input: {
									subject: "Upload issue",
									message: "The user cannot upload their recording.",
									email: "spoof@example.com",
								},
							},
						],
					}),
					{ status: 200 },
				),
			)
			.mockResolvedValueOnce(
				new Response(
					JSON.stringify({
						content: [
							{
								type: "text",
								text: "Done, I sent that to the team from your account email.",
							},
						],
					}),
					{ status: 200 },
				),
			);
		vi.stubGlobal("fetch", fetchMock);

		const execute = vi.fn().mockResolvedValue({
			status: "sent" as const,
			remainingToday: 1,
		});
		const { generateMessengerAgentReply } = await import(
			"@/lib/messenger/agent"
		);

		const result = await generateMessengerAgentReply({
			userIdentity: "Test User <user@example.com>",
			identityTag: "user:user-123",
			query: "Please send this to support",
			history: [
				{
					role: "user",
					content: "Uploads keep failing. Please send this to support.",
				},
			],
			supportEmailTool: {
				execute,
			},
		});

		expect(result).toBe(
			"Done, I sent that to the team from your account email.",
		);
		expect(fetchMock).toHaveBeenCalledTimes(2);
		const firstBody = readFetchBody(fetchMock.mock.calls[0] ?? []);
		expect(firstBody.model).toBe("claude-sonnet-5");
		expect(firstBody.max_tokens).toBe(512);
		expect(firstBody.tools?.[0]?.name).toBe("send_support_email");
		expect(firstBody.tools?.[0]?.input_schema?.properties?.email).toBe(
			undefined,
		);
		expect(execute).toHaveBeenCalledWith({
			subject: "Upload issue",
			message: "The user cannot upload their recording.",
		});

		const secondBody = readFetchBody(fetchMock.mock.calls[1] ?? []);
		expect(secondBody.max_tokens).toBe(350);
		const toolResultMessage = secondBody.messages?.at(-1);
		expect(toolResultMessage?.role).toBe("user");
		expect(toolResultMessage?.content).toEqual([
			{
				type: "tool_result",
				tool_use_id: "tool-1",
				content:
					"Support email sent to hello@cap.so from the user's account email. Remaining sends today: 1.",
			},
		]);
	});

	it("returns a tool error result when the support email tool fails", async () => {
		const fetchMock = vi
			.fn()
			.mockResolvedValueOnce(
				new Response(
					JSON.stringify({
						content: [
							{
								type: "tool_use",
								id: "tool-1",
								name: "send_support_email",
								input: {
									subject: "Upload issue",
									message: "The user cannot upload their recording.",
								},
							},
						],
					}),
					{ status: 200 },
				),
			)
			.mockResolvedValueOnce(
				new Response(
					JSON.stringify({
						content: [
							{
								type: "text",
								text: "I couldn't send that to the team right now. Please email hello@cap.so directly.",
							},
						],
					}),
					{ status: 200 },
				),
			);
		vi.stubGlobal("fetch", fetchMock);

		const execute = vi.fn().mockRejectedValue(new Error("provider failed"));
		const { generateMessengerAgentReply } = await import(
			"@/lib/messenger/agent"
		);

		const result = await generateMessengerAgentReply({
			userIdentity: "Test User <user@example.com>",
			identityTag: "user:user-123",
			query: "Please send this to support",
			history: [
				{
					role: "user",
					content: "Uploads keep failing. Please send this to support.",
				},
			],
			supportEmailTool: {
				execute,
			},
		});

		expect(result).toBe(
			"I couldn't send that to the team right now. Please email hello@cap.so directly.",
		);

		const secondBody = readFetchBody(fetchMock.mock.calls[1] ?? []);
		const toolResultMessage = secondBody.messages?.at(-1);
		expect(toolResultMessage?.content).toEqual([
			{
				type: "tool_result",
				tool_use_id: "tool-1",
				content: "Failed to send support email.",
				is_error: true,
			},
		]);
	});
});
