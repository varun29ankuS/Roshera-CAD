#!/usr/bin/env python3
"""
Hybrid Search Demo: BM25 + Vector Search
Shows how TurboRAG combines keyword and semantic search
"""

import math
import numpy as np
from collections import defaultdict, Counter
from typing import List, Dict, Tuple
import time

class BM25:
    """BM25 implementation for text ranking"""
    
    def __init__(self, k1=1.5, b=0.75):
        self.k1 = k1
        self.b = b
        self.doc_freqs = {}
        self.doc_lengths = {}
        self.avg_doc_length = 0
        self.total_docs = 0
        self.inverted_index = defaultdict(set)
        self.documents = {}
        
    def index(self, doc_id: str, text: str):
        """Index a document"""
        tokens = self.tokenize(text)
        self.documents[doc_id] = text
        self.doc_lengths[doc_id] = len(tokens)
        
        # Update inverted index
        for token in set(tokens):
            self.inverted_index[token].add(doc_id)
        
        # Update document frequencies
        token_counts = Counter(tokens)
        for token, count in token_counts.items():
            if token not in self.doc_freqs:
                self.doc_freqs[token] = {}
            self.doc_freqs[token][doc_id] = count
        
        # Update statistics
        self.total_docs += 1
        self.avg_doc_length = sum(self.doc_lengths.values()) / self.total_docs
    
    def tokenize(self, text: str) -> List[str]:
        """Simple tokenization"""
        return text.lower().split()
    
    def search(self, query: str, k: int = 5) -> List[Tuple[str, float]]:
        """Search using BM25 ranking"""
        query_tokens = self.tokenize(query)
        scores = defaultdict(float)
        
        for token in query_tokens:
            if token not in self.inverted_index:
                continue
            
            # Calculate IDF
            df = len(self.inverted_index[token])
            idf = math.log((self.total_docs - df + 0.5) / (df + 0.5) + 1.0)
            
            # Score each document containing this term
            for doc_id in self.inverted_index[token]:
                tf = self.doc_freqs[token].get(doc_id, 0)
                doc_len = self.doc_lengths[doc_id]
                
                # BM25 formula
                norm = 1 - self.b + self.b * (doc_len / self.avg_doc_length)
                score = idf * (tf * (self.k1 + 1)) / (tf + self.k1 * norm)
                scores[doc_id] += score
        
        # Sort and return top k
        results = sorted(scores.items(), key=lambda x: x[1], reverse=True)
        return results[:k]


class VectorSearch:
    """Simple vector search using cosine similarity"""
    
    def __init__(self, dimension=768):
        self.dimension = dimension
        self.vectors = {}
        self.documents = {}
    
    def index(self, doc_id: str, text: str):
        """Index a document with mock embedding"""
        self.documents[doc_id] = text
        # Create mock embedding based on text hash
        np.random.seed(hash(text) % 2**32)
        embedding = np.random.randn(self.dimension)
        embedding = embedding / np.linalg.norm(embedding)
        self.vectors[doc_id] = embedding
    
    def search(self, query: str, k: int = 5) -> List[Tuple[str, float]]:
        """Search using cosine similarity"""
        # Create query embedding
        np.random.seed(hash(query) % 2**32)
        query_vec = np.random.randn(self.dimension)
        query_vec = query_vec / np.linalg.norm(query_vec)
        
        scores = []
        for doc_id, doc_vec in self.vectors.items():
            similarity = np.dot(query_vec, doc_vec)
            scores.append((doc_id, similarity))
        
        scores.sort(key=lambda x: x[1], reverse=True)
        return scores[:k]


class HybridSearch:
    """Combines BM25 and Vector search"""
    
    def __init__(self, alpha=0.5):
        self.bm25 = BM25()
        self.vector_search = VectorSearch()
        self.alpha = alpha  # Weight for BM25
        
    def index(self, doc_id: str, text: str):
        """Index in both systems"""
        self.bm25.index(doc_id, text)
        self.vector_search.index(doc_id, text)
    
    def search(self, query: str, k: int = 5) -> List[Dict]:
        """Hybrid search combining BM25 and vector scores"""
        # Get results from both
        bm25_results = self.bm25.search(query, k * 2)
        vector_results = self.vector_search.search(query, k * 2)
        
        # Normalize scores
        max_bm25 = max([s for _, s in bm25_results]) if bm25_results else 1.0
        max_vector = max([s for _, s in vector_results]) if vector_results else 1.0
        
        # Combine scores
        combined_scores = {}
        
        for doc_id, score in bm25_results:
            normalized = score / max_bm25 if max_bm25 > 0 else 0
            combined_scores[doc_id] = {
                'bm25': normalized,
                'vector': 0,
                'text': self.bm25.documents[doc_id]
            }
        
        for doc_id, score in vector_results:
            normalized = score / max_vector if max_vector > 0 else 0
            if doc_id not in combined_scores:
                combined_scores[doc_id] = {
                    'bm25': 0,
                    'vector': normalized,
                    'text': self.vector_search.documents[doc_id]
                }
            else:
                combined_scores[doc_id]['vector'] = normalized
        
        # Calculate hybrid scores
        results = []
        for doc_id, scores in combined_scores.items():
            hybrid_score = (self.alpha * scores['bm25'] + 
                          (1 - self.alpha) * scores['vector'])
            results.append({
                'doc_id': doc_id,
                'hybrid_score': hybrid_score,
                'bm25_score': scores['bm25'],
                'vector_score': scores['vector'],
                'text': scores['text'][:100] + '...'
            })
        
        results.sort(key=lambda x: x['hybrid_score'], reverse=True)
        return results[:k]


def main():
    print("=" * 60)
    print("      Hybrid Search Demo: BM25 + Vector Search")
    print("=" * 60)
    
    # Create hybrid search engine
    hybrid = HybridSearch(alpha=0.6)  # Slightly favor BM25 for code search
    
    # Index sample Roshera CAD code snippets
    documents = [
        ("geometry_1", "Boolean operations for B-Rep models implementing union intersection and difference operations"),
        ("geometry_2", "NURBS surface evaluation with derivatives and basis function computation for CAD modeling"),
        ("geometry_3", "Topology validation checking manifold properties and Euler characteristic preservation"),
        ("timeline_1", "Timeline engine with event sourcing and branch management for design history"),
        ("timeline_2", "Operation replay system for timeline reconstruction and undo redo functionality"),
        ("ai_1", "Natural language processing for CAD commands using Whisper ASR and LLaMA models"),
        ("ai_2", "Vision pipeline for multimodal LLM integration with Three.js viewport capture"),
        ("search_1", "Vamana graph-based vector search with scalar quantization for memory efficiency"),
        ("search_2", "BM25 text ranking algorithm for keyword search with term frequency normalization"),
        ("security_1", "Enterprise ACL system with role-based access control and audit logging"),
    ]
    
    print("\n[INDEXING] Sample documents...")
    for doc_id, text in documents:
        hybrid.index(doc_id, text)
    print(f"Indexed {len(documents)} documents")
    
    # Test queries
    queries = [
        "boolean operations",
        "timeline history",
        "vector search algorithm",
        "NURBS CAD",
        "access control security",
    ]
    
    print("\n" + "=" * 60)
    print("Testing Hybrid Search")
    print("=" * 60)
    
    for query in queries:
        print(f"\nQuery: '{query}'")
        print("-" * 40)
        
        start = time.time()
        results = hybrid.search(query, k=3)
        search_time = (time.time() - start) * 1000
        
        for i, result in enumerate(results, 1):
            print(f"\n  {i}. {result['doc_id']}")
            print(f"     Hybrid Score: {result['hybrid_score']:.3f}")
            print(f"     BM25: {result['bm25_score']:.3f}, Vector: {result['vector_score']:.3f}")
            print(f"     Text: {result['text']}")
        
        print(f"\n  Search time: {search_time:.2f}ms")
    
    # Compare pure BM25 vs pure Vector vs Hybrid
    print("\n" + "=" * 60)
    print("Comparison: BM25 vs Vector vs Hybrid")
    print("=" * 60)
    
    test_query = "NURBS surface evaluation"
    print(f"\nQuery: '{test_query}'")
    
    # Pure BM25
    print("\n[BM25 Only]")
    bm25_results = hybrid.bm25.search(test_query, 3)
    for doc_id, score in bm25_results:
        print(f"  {doc_id}: {score:.3f}")
    
    # Pure Vector
    print("\n[Vector Only]")
    vector_results = hybrid.vector_search.search(test_query, 3)
    for doc_id, score in vector_results:
        print(f"  {doc_id}: {score:.3f}")
    
    # Hybrid
    print("\n[Hybrid (BM25 + Vector)]")
    hybrid_results = hybrid.search(test_query, 3)
    for result in hybrid_results:
        print(f"  {result['doc_id']}: {result['hybrid_score']:.3f}")
    
    print("\n" + "=" * 60)
    print("Key Insights:")
    print("  - BM25 excels at exact keyword matching")
    print("  - Vector search finds semantically similar content")
    print("  - Hybrid combines both for best results")
    print("  - TurboRAG uses this at scale with 5000+ documents")
    print("=" * 60)


if __name__ == "__main__":
    main()