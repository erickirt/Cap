import { db } from "@cap/database";
import { videos, videoUploads } from "@cap/database/schema";
import type { S3Bucket, Video } from "@cap/web-domain";
import { and, asc, eq, isNotNull, like, sql } from "drizzle-orm";
import { start } from "workflow/api";
import { setVideoProcessingError } from "@/lib/video-processing";
import { processVideoWorkflow } from "@/workflows/process-video";

export const WORKFLOW_UPGRADE_ERROR_FRAGMENT =
	"This Workflow 4.x beta release is no longer supported";
export const VIDEO_PROCESSING_RECOVERY_BATCH_SIZE = 20;

export type FailedVideoProcessingRecoveryStatus =
	| "started"
	| "already-claimed"
	| "retry-scheduled"
	| "missing-source"
	| "failed";

export type FailedVideoProcessingRecoverySummary = {
	checked: number;
	statuses: Partial<Record<FailedVideoProcessingRecoveryStatus, number>>;
	results: Array<{
		videoId: Video.VideoId;
		status: FailedVideoProcessingRecoveryStatus;
	}>;
};

const getAffectedRows = (result: unknown) => {
	if (Array.isArray(result)) {
		return (
			(result[0] as { affectedRows?: number } | undefined)?.affectedRows ?? 0
		);
	}

	return (result as { affectedRows?: number } | undefined)?.affectedRows ?? 0;
};

async function recoverCandidate({
	videoId,
	userId,
	rawFileKey,
	bucketId,
}: {
	videoId: Video.VideoId;
	userId: string;
	rawFileKey: string;
	bucketId: S3Bucket.S3BucketId | null;
}): Promise<FailedVideoProcessingRecoveryStatus> {
	const claimResult = await db()
		.update(videoUploads)
		.set({
			phase: "processing",
			processingProgress: 0,
			processingMessage: "Processing video",
			processingError: null,
			updatedAt: new Date(),
		})
		.where(
			and(
				eq(videoUploads.videoId, videoId),
				eq(videoUploads.phase, "error"),
				eq(videoUploads.rawFileKey, rawFileKey),
				like(
					videoUploads.processingError,
					`%${WORKFLOW_UPGRADE_ERROR_FRAGMENT}%`,
				),
			),
		);

	if (getAffectedRows(claimResult) === 0) {
		return "already-claimed";
	}

	try {
		await start(processVideoWorkflow, [
			{
				videoId,
				userId,
				rawFileKey,
				bucketId,
			},
		]);
		return "started";
	} catch (error) {
		const message = error instanceof Error ? error.message : String(error);
		await setVideoProcessingError(
			videoId,
			"Processing recovery will retry automatically",
			new Error(
				`${WORKFLOW_UPGRADE_ERROR_FRAGMENT}; recovery start failed: ${message}`,
			),
		);
		return "retry-scheduled";
	}
}

export async function recoverFailedVideoProcessing({
	limit = VIDEO_PROCESSING_RECOVERY_BATCH_SIZE,
}: {
	limit?: number;
} = {}): Promise<FailedVideoProcessingRecoverySummary> {
	const candidates = await db()
		.select({
			videoId: videos.id,
			userId: videos.ownerId,
			bucketId: videos.bucket,
			rawFileKey: videoUploads.rawFileKey,
		})
		.from(videos)
		.innerJoin(videoUploads, eq(videos.id, videoUploads.videoId))
		.where(
			and(
				sql`JSON_UNQUOTE(JSON_EXTRACT(${videos.source}, '$.type')) = 'webMP4'`,
				eq(videoUploads.phase, "error"),
				isNotNull(videoUploads.rawFileKey),
				like(
					videoUploads.processingError,
					`%${WORKFLOW_UPGRADE_ERROR_FRAGMENT}%`,
				),
			),
		)
		.orderBy(asc(videoUploads.updatedAt))
		.limit(limit);

	const summary: FailedVideoProcessingRecoverySummary = {
		checked: candidates.length,
		statuses: {},
		results: [],
	};

	for (const candidate of candidates) {
		let status: FailedVideoProcessingRecoveryStatus;
		if (!candidate.rawFileKey) {
			status = "missing-source";
		} else {
			try {
				status = await recoverCandidate({
					videoId: candidate.videoId,
					userId: candidate.userId,
					rawFileKey: candidate.rawFileKey,
					bucketId: candidate.bucketId,
				});
			} catch (error) {
				status = "failed";
				console.error(
					`[video-processing-recovery] Failed to recover ${candidate.videoId}:`,
					error,
				);
			}
		}

		summary.statuses[status] = (summary.statuses[status] ?? 0) + 1;
		summary.results.push({
			videoId: candidate.videoId,
			status,
		});
	}

	return summary;
}
