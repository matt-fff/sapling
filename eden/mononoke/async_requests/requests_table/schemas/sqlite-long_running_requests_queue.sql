/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This software may be used and distributed according to the terms of the
 * GNU General Public License version 2.
 */

 CREATE TABLE IF NOT EXISTS `long_running_request_queue` (
  `id` INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL,
  `repo_id` INTEGER DEFAULT NULL,
  `bookmark` VARCHAR(512) DEFAULT NULL,
  `request_type` VARCHAR(255) NOT NULL,
  `args_blobstore_key` VARCHAR(255) NOT NULL,
  `result_blobstore_key` VARCHAR(255) DEFAULT NULL,
  `created_at` BIGINT(20) NOT NULL,
  `started_processing_at` BIGINT(20) DEFAULT NULL,
  `inprogress_last_updated_at` BIGINT(20) DEFAULT NULL,
  `ready_at` BIGINT(20) DEFAULT NULL,
  `polled_at` BIGINT(20) DEFAULT NULL,
  `status` VARCHAR(32) NOT NULL, -- enum('new','inprogress','ready','polled','failed') NOT NULL DEFAULT 'new',
  `claimed_by` VARCHAR(255) NULL,
  `num_retries` TINYINT UNSIGNED DEFAULT NULL,
  `failed_at` BIGINT(20) DEFAULT NULL
);

CREATE INDEX IF NOT EXISTS `long_running_request_queue_request_status` ON `long_running_request_queue` (`status`, `request_type`);
CREATE INDEX IF NOT EXISTS `long_running_request_queue_request_creation` ON `long_running_request_queue` (`created_at`);
CREATE INDEX IF NOT EXISTS `long_running_request_queue_request_dequeue` ON `long_running_request_queue` (`status`, `repo_id`, `created_at`);
CREATE INDEX IF NOT EXISTS `long_running_request_queue_abandoned_request_index` ON `long_running_request_queue` (`repo_id`, `status`, `inprogress_last_updated_at`);
CREATE INDEX IF NOT EXISTS `long_running_request_queue_abandoned_request_index_any` ON `long_running_request_queue` (`status`, `inprogress_last_updated_at`);
CREATE INDEX IF NOT EXISTS `long_running_request_queue_list_requests` ON `long_running_request_queue` (`status`, `repo_id`, `inprogress_last_updated_at`, `created_at`);
CREATE INDEX IF NOT EXISTS `long_running_request_queue_list_requests_any` ON `long_running_request_queue` (`status`, `inprogress_last_updated_at`, `created_at`);
