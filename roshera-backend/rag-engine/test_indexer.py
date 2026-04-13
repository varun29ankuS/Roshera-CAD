#!/usr/bin/env python3
"""
Test indexer for Roshera-CAD codebase
Demonstrates the indexing approach without full Rust compilation
"""

import os
import time
import hashlib
from pathlib import Path
from collections import defaultdict
from typing import List, Dict, Tuple
import re

class CodebaseIndexer:
    def __init__(self, chunk_size=100, workers=4):
        self.chunk_size = chunk_size
        self.workers = workers
        self.stats = defaultdict(int)
        self.chunks = []
        self.file_types = defaultdict(int)
        
        # Patterns to ignore
        self.ignore_patterns = [
            'target', 'node_modules', '.git', 'dist', 'build',
            '__pycache__', '.pyc', '.exe', '.dll', '.so', '.wasm'
        ]
        
    def should_ignore(self, path: Path) -> bool:
        """Check if path should be ignored"""
        path_str = str(path)
        for pattern in self.ignore_patterns:
            if pattern in path_str:
                return True
        return False
    
    def get_file_type(self, path: Path) -> str:
        """Determine file type from extension"""
        ext = path.suffix.lower()
        mapping = {
            '.rs': 'Rust',
            '.py': 'Python',
            '.js': 'JavaScript',
            '.ts': 'TypeScript',
            '.tsx': 'TypeScript',
            '.java': 'Java',
            '.cpp': 'C++',
            '.cc': 'C++',
            '.h': 'C++',
            '.hpp': 'C++',
            '.go': 'Go',
            '.md': 'Markdown',
            '.toml': 'TOML',
            '.json': 'JSON',
            '.yaml': 'YAML',
            '.yml': 'YAML',
            '.txt': 'Text',
            '.html': 'HTML',
            '.css': 'CSS',
        }
        return mapping.get(ext, 'Unknown')
    
    def extract_functions(self, content: str, file_type: str) -> List[str]:
        """Extract function names from code"""
        functions = []
        
        if file_type == 'Rust':
            # Find Rust functions
            pattern = r'(?:pub\s+)?(?:async\s+)?fn\s+(\w+)'
            functions.extend(re.findall(pattern, content))
        elif file_type == 'Python':
            # Find Python functions
            pattern = r'def\s+(\w+)'
            functions.extend(re.findall(pattern, content))
        elif file_type in ['JavaScript', 'TypeScript']:
            # Find JS/TS functions
            pattern = r'(?:function\s+(\w+)|(?:const|let|var)\s+(\w+)\s*=\s*(?:async\s+)?\()'
            for match in re.finditer(pattern, content):
                func = match.group(1) or match.group(2)
                if func:
                    functions.append(func)
        
        return functions
    
    def create_chunks(self, path: Path, content: str) -> List[Dict]:
        """Create chunks from file content"""
        chunks = []
        lines = content.split('\n')
        file_type = self.get_file_type(path)
        
        # Extract functions for smart chunking
        functions = self.extract_functions(content, file_type)
        
        # Create line-based chunks
        for i in range(0, len(lines), self.chunk_size - 20):  # 20 line overlap
            chunk_lines = lines[i:i + self.chunk_size]
            chunk_content = '\n'.join(chunk_lines)
            
            # Find which functions are in this chunk
            chunk_functions = []
            for func in functions:
                if func in chunk_content:
                    chunk_functions.append(func)
            
            chunk = {
                'file': str(path),
                'file_type': file_type,
                'start_line': i + 1,
                'end_line': min(i + self.chunk_size, len(lines)),
                'content': chunk_content,
                'functions': chunk_functions,
                'hash': hashlib.md5(chunk_content.encode()).hexdigest()[:8],
            }
            chunks.append(chunk)
        
        return chunks
    
    def index_file(self, path: Path) -> None:
        """Index a single file"""
        try:
            # Check file size
            if path.stat().st_size > 10 * 1024 * 1024:  # Skip files > 10MB
                return
            
            # Read content
            with open(path, 'r', encoding='utf-8', errors='ignore') as f:
                content = f.read()
            
            # Get file type
            file_type = self.get_file_type(path)
            self.file_types[file_type] += 1
            
            # Create chunks
            chunks = self.create_chunks(path, content)
            self.chunks.extend(chunks)
            
            # Update stats
            self.stats['files'] += 1
            self.stats['chunks'] += len(chunks)
            self.stats['bytes'] += len(content)
            
            # Progress indicator
            if self.stats['files'] % 10 == 0:
                print(f"  Indexed {self.stats['files']} files...", end='\r')
                
        except Exception as e:
            self.stats['errors'] += 1
    
    def index_directory(self, root_path: str) -> Dict:
        """Index entire directory"""
        root = Path(root_path)
        print(f"\n[INDEXING] {root}")
        print("=" * 60)
        
        # Collect all files
        all_files = []
        for path in root.rglob('*'):
            if path.is_file() and not self.should_ignore(path):
                all_files.append(path)
        
        print(f"[FOUND] {len(all_files)} files to index")
        
        # Index files
        start_time = time.time()
        for file_path in all_files:
            self.index_file(file_path)
        
        duration = time.time() - start_time
        
        # Final stats
        print(f"\n\n[COMPLETE] Indexing Complete!")
        print("=" * 60)
        print(f"Statistics:")
        print(f"  - Files indexed: {self.stats['files']}")
        print(f"  - Chunks created: {self.stats['chunks']}")
        print(f"  - Total size: {self.stats['bytes'] / 1_048_576:.2f} MB")
        print(f"  - Time taken: {duration:.2f} seconds")
        print(f"  - Files/second: {self.stats['files'] / duration:.1f}")
        print(f"  - Errors: {self.stats['errors']}")
        
        print(f"\nFile types:")
        sorted_types = sorted(self.file_types.items(), key=lambda x: x[1], reverse=True)
        for file_type, count in sorted_types[:10]:
            bar = '#' * int(count / max(self.file_types.values()) * 30)
            print(f"  {file_type:12} {bar} {count}")
        
        # Sample chunks
        print(f"\nSample chunks:")
        for chunk in self.chunks[:3]:
            print(f"\n  File: {chunk['file']}")
            print(f"  Lines: {chunk['start_line']}-{chunk['end_line']}")
            print(f"  Functions: {', '.join(chunk['functions'][:3]) if chunk['functions'] else 'none'}")
            print(f"  Preview: {chunk['content'][:100]}...")
        
        return self.stats
    
    def search(self, query: str, limit: int = 5) -> List[Dict]:
        """Simple keyword search"""
        results = []
        query_lower = query.lower()
        
        for chunk in self.chunks:
            if query_lower in chunk['content'].lower():
                # Calculate simple relevance score
                score = chunk['content'].lower().count(query_lower)
                results.append((score, chunk))
        
        # Sort by relevance
        results.sort(key=lambda x: x[0], reverse=True)
        
        return [chunk for _, chunk in results[:limit]]


def main():
    print("=" * 60)
    print("         TurboRAG Codebase Indexer (Python Demo)")
    print("=" * 60)
    
    # Create indexer
    indexer = CodebaseIndexer(chunk_size=100, workers=4)
    
    # Index Roshera-CAD
    codebase_path = r"C:\Users\Varun Sharma\Roshera-CAD\roshera-backend"
    stats = indexer.index_directory(codebase_path)
    
    # Test search
    print("\n\n[SEARCH] Testing Search:")
    print("=" * 60)
    
    queries = [
        "geometry engine",
        "async function",
        "boolean operations",
        "NURBS",
        "timeline",
    ]
    
    for query in queries:
        print(f"\nSearching for: '{query}'")
        results = indexer.search(query, limit=2)
        print(f"  Found {len(results)} results")
        
        for i, result in enumerate(results, 1):
            print(f"\n  Result {i}:")
            print(f"    File: {result['file']}")
            print(f"    Lines: {result['start_line']}-{result['end_line']}")
            preview = result['content'][:150].replace('\n', ' ')
            print(f"    Preview: {preview}...")
    
    print("\n\n[DONE] Demo complete! This is what TurboRAG does at enterprise scale.")
    print("   The Rust version is 100x faster with vector search!")

if __name__ == "__main__":
    main()