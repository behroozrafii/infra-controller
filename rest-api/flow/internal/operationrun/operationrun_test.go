// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

package operationrun

import (
	"testing"

	"github.com/stretchr/testify/require"

	taskcommon "github.com/NVIDIA/infra-controller/rest-api/flow/internal/task/common"
)

func TestOperationRunStatusIsTerminalIncludesCompletedWithFailures(t *testing.T) {
	require.True(t, OperationRunStatusCompletedWithFailures.IsTerminal())
}

func TestOperationRunStatusMessage(t *testing.T) {
	tests := []struct {
		status OperationRunStatus
		want   string
	}{
		{
			status: OperationRunStatusPending,
			want:   "operation run pending",
		},
		{
			status: OperationRunStatusRunning,
			want:   "operation run running",
		},
		{
			status: OperationRunStatusPaused,
			want:   "operation run paused",
		},
		{
			status: OperationRunStatusCompleted,
			want:   "operation run completed",
		},
		{
			status: OperationRunStatusCompletedWithFailures,
			want:   "operation run completed with failed targets",
		},
		{
			status: OperationRunStatusCancelled,
			want:   "operation run cancelled",
		},
		{
			status: OperationRunStatusFailed,
			want:   "operation run failed",
		},
		{
			status: OperationRunStatus("unknown"),
		},
	}

	for _, tt := range tests {
		t.Run(string(tt.status), func(t *testing.T) {
			require.Equal(t, tt.want, tt.status.Message())
		})
	}
}

func TestOperationRunCanPause(t *testing.T) {
	tests := []struct {
		name string
		run  *OperationRun
		want bool
	}{
		{
			name: "pending",
			run:  &OperationRun{Status: OperationRunStatusPending},
			want: true,
		},
		{
			name: "running",
			run:  &OperationRun{Status: OperationRunStatusRunning},
			want: true,
		},
		{
			name: "paused",
			run:  &OperationRun{Status: OperationRunStatusPaused},
			want: true,
		},
		{
			name: "completed",
			run:  &OperationRun{Status: OperationRunStatusCompleted},
		},
		{
			name: "nil",
			run:  nil,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			require.Equal(t, tt.want, tt.run.CanPause())
		})
	}
}

func TestOperationRunCanResume(t *testing.T) {
	tests := []struct {
		name string
		run  *OperationRun
		want bool
	}{
		{
			name: "operator paused",
			run: &OperationRun{
				Status:       OperationRunStatusPaused,
				StatusReason: OperationRunStatusReasonOperatorPaused,
			},
			want: true,
		},
		{
			name: "safety paused",
			run: &OperationRun{
				Status:       OperationRunStatusPaused,
				StatusReason: OperationRunStatusReasonSafetyGate,
			},
			want: true,
		},
		{
			name: "phase gate paused",
			run: &OperationRun{
				Status:       OperationRunStatusPaused,
				StatusReason: OperationRunStatusReasonPhaseGate,
			},
		},
		{
			name: "running",
			run:  &OperationRun{Status: OperationRunStatusRunning},
		},
		{
			name: "nil",
			run:  nil,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			require.Equal(t, tt.want, tt.run.CanResume())
		})
	}
}

func TestOperationRunCanAdvancePhase(t *testing.T) {
	tests := []struct {
		name string
		run  *OperationRun
		want bool
	}{
		{
			name: "phase gate paused",
			run: &OperationRun{
				Status:       OperationRunStatusPaused,
				StatusReason: OperationRunStatusReasonPhaseGate,
			},
			want: true,
		},
		{
			name: "operator paused",
			run: &OperationRun{
				Status:       OperationRunStatusPaused,
				StatusReason: OperationRunStatusReasonOperatorPaused,
			},
		},
		{
			name: "running",
			run:  &OperationRun{Status: OperationRunStatusRunning},
		},
		{
			name: "nil",
			run:  nil,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			require.Equal(t, tt.want, tt.run.CanAdvancePhase())
		})
	}
}

func TestTerminalTargetStatusesMatchIsTerminal(t *testing.T) {
	terminal := map[OperationRunTargetStatus]struct{}{}
	for _, status := range TerminalTargetStatuses() {
		terminal[status] = struct{}{}
	}

	for _, status := range []OperationRunTargetStatus{
		OperationRunTargetStatusPending,
		OperationRunTargetStatusClaimed,
		OperationRunTargetStatusBlocked,
		OperationRunTargetStatusSubmitted,
		OperationRunTargetStatusCompleted,
		OperationRunTargetStatusFailed,
		OperationRunTargetStatusTerminated,
		OperationRunTargetStatusSkipped,
	} {
		_, listed := terminal[status]
		require.Equal(t, status.IsTerminal(), listed, status)
	}
}

func TestOperationRunTargetStatusFromTaskStatus(t *testing.T) {
	tests := []struct {
		name   string
		status taskcommon.TaskStatus
		want   OperationRunTargetStatus
	}{
		{
			name:   "completed",
			status: taskcommon.TaskStatusCompleted,
			want:   OperationRunTargetStatusCompleted,
		},
		{
			name:   "failed",
			status: taskcommon.TaskStatusFailed,
			want:   OperationRunTargetStatusFailed,
		},
		{
			name:   "terminated",
			status: taskcommon.TaskStatusTerminated,
			want:   OperationRunTargetStatusTerminated,
		},
		{
			name:   "non-terminal",
			status: taskcommon.TaskStatusRunning,
			want:   OperationRunTargetStatusSubmitted,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			require.Equal(
				t,
				tt.want,
				OperationRunTargetStatusFromTaskStatus(tt.status),
			)
		})
	}
}
