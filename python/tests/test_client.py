import os
from pathlib import Path
import tempfile
import unittest

from git_vdb import PersistentClient


class ClientTest(unittest.TestCase):
    def test_vector_crud_query_and_filters(self):
        with tempfile.TemporaryDirectory() as temporary:
            client = PersistentClient(Path(temporary) / "vectors.git")
            docs = client.get_or_create_collection("docs")
            docs.upsert(
                ids=["east", "north"],
                embeddings=[[1.0, 0.0], [0.0, 1.0]],
                metadatas=[{"kind": "guide"}, {"kind": "note"}],
                documents=["East guide", "North note"],
                batch_size=1,
            )
            self.assertEqual(docs.count(), 2)
            result = docs.query(
                query_embeddings=[[0.9, 0.1]],
                where={"kind": "guide"},
                n_results=1,
            )
            self.assertEqual(result["ids"], [["east"]])
            self.assertEqual(result["documents"], [["East guide"]])
            self.assertEqual(docs.get(ids=["north"])["ids"], ["north"])
            docs.delete(ids=["north"])
            self.assertEqual(docs.count(), 1)
            self.assertTrue(client.doctor()["valid"])


if __name__ == "__main__":
    unittest.main()
