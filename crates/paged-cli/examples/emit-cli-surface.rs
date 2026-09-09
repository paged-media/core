/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 *
 * This file is part of paged (https://paged.media) and is additionally
 * available under the Paged Media Enterprise License (PMEL). Full
 * copyright and license information is available in LICENSE.md which is
 * distributed with this source code.
 *
 *  @copyright  Copyright (c) And The Next GmbH
 *  @license    MPL-2.0 OR Paged Media Enterprise License (PMEL)
 */

//! Emit the CLI's own command tree as pretty JSON — the committed
//! artifact (`crates/paged-cli/cli.json`) that docs.paged.media
//! generates its command reference from, the way `catalog.json` backs
//! the scripting reference. Regenerate after any change to the tree:
//!
//! ```sh
//! cargo run -p paged-cli --example emit-cli-surface > crates/paged-cli/cli.json
//! ```
//!
//! The `cli_json_artifact_is_current` test fails if the committed file
//! drifts from the parser.

fn main() {
    let json = serde_json::to_string_pretty(&paged_cli::cli::surface())
        .expect("serialize the CLI surface");
    println!("{json}");
}
