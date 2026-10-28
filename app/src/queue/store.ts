import { create } from "zustand";

export type ProfileId = "anime" | "cg";

export type JobStage = "queued" | "analysing" | "upscaling" | "audio" | "muxing" | "done" | "failed";

export interface Job {
  id: string;
  path: string;
  profile: ProfileId;
  stage: JobStage;
}

interface QueueState {
  jobs: Job[];
  running: boolean;
  paused: boolean;
}

// Phase 0: state only. Commands and core events are wired in Phase 2.
export const useQueue = create<QueueState>(() => ({
  jobs: [],
  running: false,
  paused: false,
}));
