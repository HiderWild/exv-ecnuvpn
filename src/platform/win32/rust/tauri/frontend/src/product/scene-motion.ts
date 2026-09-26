export interface ScenePresentation {
  state: "idle" | "working" | "connected";
  stopping?: boolean;
  paused?: boolean;
  reduced?: boolean;
}
export interface SceneMotion { extension: number; phase: number }

/** 只表达连接关系，不把等待时长当作网络进度。 */
export function advanceSceneMotion(pose: SceneMotion, input: ScenePresentation, delta: number): SceneMotion {
  const target = input.stopping || input.state === "idle" ? 0 : input.state === "connected" ? 1 : 0.58;
  const still = input.reduced || input.paused;
  const next = still ? target : pose.extension + (target - pose.extension) * (1 - Math.exp(-10 * Math.min(delta, 0.1)));
  return {
    extension: Math.abs(target - next) < 0.002 ? target : next,
    phase: input.state === "working" && !input.stopping && !still
      ? (pose.phase + Math.min(delta, 0.1) * 0.45) % 1 : pose.phase,
  };
}
