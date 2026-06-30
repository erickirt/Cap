import { User } from "@cap/web-domain";
import { beforeEach, describe, expect, it, vi } from "vitest";

const whereMock = vi.hoisted(() => vi.fn());
const valuesMock = vi.hoisted(() => vi.fn());
const sendEmailMock = vi.hoisted(() => vi.fn());
const messengerSupportEmailMock = vi.hoisted(() => vi.fn(() => null));

const mockDb = vi.hoisted(() => ({
	select: vi.fn(() => mockDb),
	from: vi.fn(() => mockDb),
	where: whereMock,
	insert: vi.fn(() => mockDb),
	values: valuesMock,
}));

vi.mock("server-only", () => ({}));

vi.mock("@cap/database", () => ({
	db: vi.fn(() => mockDb),
}));

vi.mock("@cap/database/helpers", () => ({
	nanoId: vi.fn(() => "support-email-123"),
}));

vi.mock("@cap/database/schema", () => ({
	messengerSupportEmails: {
		userId: "userId",
		createdAt: "createdAt",
	},
}));

vi.mock("@cap/database/emails/config", () => ({
	sendEmail: sendEmailMock,
}));

vi.mock("@cap/database/emails/messenger-support-email", () => ({
	MessengerSupportEmail: messengerSupportEmailMock,
}));

vi.mock("drizzle-orm", () => ({
	and: vi.fn((...conditions: unknown[]) => ({ type: "and", conditions })),
	count: vi.fn(() => "count"),
	eq: vi.fn((left: unknown, right: unknown) => ({ type: "eq", left, right })),
	gte: vi.fn((left: unknown, right: unknown) => ({ type: "gte", left, right })),
}));

describe("sendMessengerSupportEmail", () => {
	beforeEach(() => {
		vi.clearAllMocks();
		valuesMock.mockResolvedValue(undefined);
		sendEmailMock.mockResolvedValue(undefined);
	});

	it("sends support email from account context and records the send", async () => {
		whereMock.mockResolvedValueOnce([{ value: 1 }]);
		const { sendMessengerSupportEmail } = await import(
			"@/lib/messenger/support-email"
		);

		const result = await sendMessengerSupportEmail({
			user: {
				id: User.UserId.make("user-123"),
				email: "user@example.com",
				name: "Test User",
			},
			conversationId: "conversation-123",
			subject: "  Upload\nissue  ",
			message: "  Uploads keep failing.  ",
			now: new Date("2026-06-30T18:00:00.000Z"),
		});

		expect(result).toEqual({
			status: "sent",
			remainingToday: 0,
		});
		expect(messengerSupportEmailMock).toHaveBeenCalledWith({
			userEmail: "user@example.com",
			userName: "Test User",
			conversationId: "conversation-123",
			message: "Uploads keep failing.",
		});
		expect(sendEmailMock).toHaveBeenCalledWith(
			expect.objectContaining({
				email: "hello@cap.so",
				subject: "Messenger support: Upload issue",
				replyTo: "user@example.com",
				fromOverride: "Cap Support <richie@send.cap.so>",
			}),
		);
		expect(valuesMock).toHaveBeenCalledWith(
			expect.objectContaining({
				id: "support-email-123",
				conversationId: "conversation-123",
				userId: "user-123",
				userEmail: "user@example.com",
				subject: "Upload issue",
				message: "Uploads keep failing.",
				createdAt: new Date("2026-06-30T18:00:00.000Z"),
			}),
		);
	});

	it("does not send after two support emails in the same UTC day", async () => {
		whereMock.mockResolvedValueOnce([{ value: 2 }]);
		const { sendMessengerSupportEmail } = await import(
			"@/lib/messenger/support-email"
		);

		const result = await sendMessengerSupportEmail({
			user: {
				id: User.UserId.make("user-123"),
				email: "user@example.com",
				name: "Test User",
			},
			conversationId: "conversation-123",
			subject: "Upload issue",
			message: "Uploads keep failing.",
			now: new Date("2026-06-30T18:00:00.000Z"),
		});

		expect(result).toEqual({
			status: "rate_limited",
			remainingToday: 0,
		});
		expect(sendEmailMock).not.toHaveBeenCalled();
		expect(valuesMock).not.toHaveBeenCalled();
	});
});
