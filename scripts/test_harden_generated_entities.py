import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))

from harden_generated_entities import harden_model


UNSAFE_ENTITY = """\
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "credential_records")]
pub struct Model {
    pub id: i32,
    pub api_key: String,
    pub workspace_api_token: String,
    pub provider_service_key: String,
    pub password: String,
    pub authorization: String,
    pub oauth_access_token: String,
    pub public_key: String,
}
"""


class HardenGeneratedEntitiesTest(unittest.TestCase):
    def test_hardens_multiple_suspicious_fields_and_preserves_generic_names(self):
        hardened = harden_model(UNSAFE_ENTITY)

        self.assertNotIn("Debug", hardened.split("pub struct Model", 1)[0])
        for field in (
            "api_key",
            "workspace_api_token",
            "provider_service_key",
            "password",
            "authorization",
            "oauth_access_token",
        ):
            self.assertIn(f"#[serde(skip_serializing)]\n    pub {field}:", hardened)
        self.assertNotIn(f"#[serde(skip_serializing)]\n    pub public_key:", hardened)

    def test_hardening_is_idempotent(self):
        hardened = harden_model(UNSAFE_ENTITY)

        self.assertEqual(hardened, harden_model(hardened))

    def test_unsupported_shape_fails_closed(self):
        missing_derive = UNSAFE_ENTITY.replace(
            "#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]\n",
            "",
        )
        unsupported_derive = UNSAFE_ENTITY.replace(
            "DeriveEntityModel, ",
            "",
        )

        with self.assertRaisesRegex(ValueError, "Model derive is missing"):
            harden_model(missing_derive)
        with self.assertRaisesRegex(ValueError, "Model derive is missing or unsupported"):
            harden_model(unsupported_derive)


if __name__ == "__main__":
    unittest.main()
