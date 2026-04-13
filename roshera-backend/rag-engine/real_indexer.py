#!/usr/bin/env python3
"""
Real File Indexer - Actually indexes your codebase files
"""

import os
import sys
import json
import time
import hashlib
from pathlib import Path
from typing import List, Dict, Optional
from collections import defaultdict

# Set UTF-8 encoding for Windows
if sys.platform == "win32":
    import io
    sys.stdout = io.TextIOWrapper(sys.stdout.buffer, encoding='utf-8')
    sys.stderr = io.TextIOWrapper(sys.stderr.buffer, encoding='utf-8')

class RealFileIndexer:
    def __init__(self, root_path: str = None):
        self.root_path = Path(root_path) if root_path else Path.cwd().parent.parent  # Go up to Roshera-CAD
        self.chunks = []
        self.file_index = defaultdict(list)
        self.symbol_index = defaultdict(list)
        self.stats = {
            'total_files': 0,
            'total_chunks': 0,
            'total_bytes': 0,
            'file_types': defaultdict(int),
            'errors': []
        }
        
    def index_directory(self, path: Path = None):
        """Actually index all files in the directory"""
        if path is None:
            path = self.root_path
            
        print(f"🔍 Starting REAL indexing of: {path}")
        print(f"   This will read actual files from your disk...")
        print("-"*70)
        
        start_time = time.time()
        
        # Walk through all files
        for root, dirs, files in os.walk(path):
            # Skip certain directories
            dirs[:] = [d for d in dirs if d not in {'.git', 'node_modules', 'target', '__pycache__', 'dist', 'build'}]
            
            for file in files:
                file_path = Path(root) / file
                
                # Skip non-code files
                if file_path.suffix not in {'.rs', '.py', '.js', '.ts', '.tsx', '.java', '.cpp', '.h', '.hpp', '.go', '.md', '.toml', '.json', '.yaml', '.yml'}:
                    continue
                    
                # Skip large files
                try:
                    if file_path.stat().st_size > 1024 * 1024:  # 1MB
                        continue
                except:
                    continue
                    
                self.index_file(file_path)
                
        elapsed = time.time() - start_time
        
        print("\n" + "="*70)
        print(f"✅ REAL INDEXING COMPLETE!")
        print(f"   Files indexed: {self.stats['total_files']}")
        print(f"   Total chunks: {self.stats['total_chunks']}")
        print(f"   Total size: {self.stats['total_bytes'] / 1024 / 1024:.2f} MB")
        print(f"   Time taken: {elapsed:.2f} seconds")
        print("\n   File types indexed:")
        for ext, count in sorted(self.stats['file_types'].items()):
            print(f"     {ext}: {count} files")
        if self.stats['errors']:
            print(f"\n   ⚠️  {len(self.stats['errors'])} errors occurred")
        print("="*70)
        
    def index_file(self, file_path: Path):
        """Index a single file by actually reading it"""
        try:
            # Read the actual file content
            with open(file_path, 'r', encoding='utf-8', errors='ignore') as f:
                content = f.read()
                
            if not content.strip():
                return
                
            self.stats['total_files'] += 1
            self.stats['total_bytes'] += len(content)
            self.stats['file_types'][file_path.suffix] += 1
            
            # Create chunks from actual content
            lines = content.split('\n')
            chunk_size = 50  # lines per chunk
            
            for i in range(0, len(lines), chunk_size - 10):  # 10 line overlap
                chunk_lines = lines[i:i + chunk_size]
                chunk_content = '\n'.join(chunk_lines)
                
                if len(chunk_content.strip()) < 20:  # Skip tiny chunks
                    continue
                    
                # Extract symbols from actual code
                symbols = self.extract_symbols(chunk_content, file_path.suffix)
                
                chunk = {
                    'id': self.stats['total_chunks'],
                    'file': str(file_path.relative_to(self.root_path)),
                    'content': chunk_content,
                    'start_line': i + 1,
                    'end_line': min(i + chunk_size, len(lines)),
                    'symbols': symbols,
                    'file_type': file_path.suffix,
                    'size': len(chunk_content)
                }
                
                self.chunks.append(chunk)
                self.file_index[str(file_path)].append(chunk['id'])
                
                for symbol in symbols:
                    self.symbol_index[symbol.lower()].append(chunk['id'])
                    
                self.stats['total_chunks'] += 1
                
            print(f"   ✓ Indexed: {file_path.name} ({self.stats['total_chunks']} chunks total)")
            
        except Exception as e:
            self.stats['errors'].append(f"{file_path}: {str(e)}")
            
    def extract_symbols(self, content: str, file_type: str) -> List[str]:
        """Extract function/class/variable names from code"""
        symbols = []
        
        # Language-specific patterns
        patterns = {
            '.rs': [
                r'fn\s+(\w+)',
                r'struct\s+(\w+)',
                r'impl\s+(\w+)',
                r'enum\s+(\w+)',
                r'trait\s+(\w+)',
                r'pub\s+fn\s+(\w+)',
                r'pub\s+struct\s+(\w+)',
            ],
            '.py': [
                r'def\s+(\w+)',
                r'class\s+(\w+)',
                r'async\s+def\s+(\w+)',
            ],
            '.js': [
                r'function\s+(\w+)',
                r'class\s+(\w+)',
                r'const\s+(\w+)',
                r'let\s+(\w+)',
                r'var\s+(\w+)',
            ],
            '.ts': [
                r'function\s+(\w+)',
                r'class\s+(\w+)',
                r'interface\s+(\w+)',
                r'type\s+(\w+)',
                r'const\s+(\w+)',
            ],
        }
        
        import re
        
        # Get patterns for this file type
        file_patterns = patterns.get(file_type, [])
        
        for pattern in file_patterns:
            matches = re.findall(pattern, content)
            symbols.extend(matches)
            
        # Also extract obvious CamelCase/snake_case identifiers
        # Find PascalCase
        symbols.extend(re.findall(r'\b([A-Z][a-z]+(?:[A-Z][a-z]+)+)\b', content))
        # Find snake_case functions
        symbols.extend(re.findall(r'\b([a-z]+(?:_[a-z]+)+)\s*\(', content))
        
        # Deduplicate
        return list(set(symbols))[:10]  # Keep top 10 symbols per chunk
        
    def search(self, query: str, limit: int = 10) -> List[Dict]:
        """Search through actually indexed content"""
        query_lower = query.lower()
        results = []
        
        for chunk in self.chunks:
            score = 0.0
            match_types = []
            
            # Exact match in content
            if query_lower in chunk['content'].lower():
                score += 5.0
                match_types.append('exact')
                
            # Symbol match
            for symbol in chunk['symbols']:
                if query_lower in symbol.lower():
                    score += 3.0
                    if 'symbol' not in match_types:
                        match_types.append('symbol')
                        
            # Word match
            query_words = query_lower.split()
            content_lower = chunk['content'].lower()
            for word in query_words:
                if len(word) > 2 and word in content_lower:
                    score += 1.0
                    if 'keyword' not in match_types:
                        match_types.append('keyword')
                        
            if score > 0:
                results.append({
                    'chunk': chunk,
                    'score': score,
                    'match_types': match_types
                })
                
        # Sort by score
        results.sort(key=lambda x: x['score'], reverse=True)
        
        return results[:limit]
        
    def save_index(self, path: str = "index.json"):
        """Save the index to disk"""
        with open(path, 'w', encoding='utf-8') as f:
            json.dump({
                'chunks': self.chunks,
                'stats': dict(self.stats),
                'root_path': str(self.root_path)
            }, f, indent=2)
        print(f"💾 Index saved to {path}")
        
    def load_index(self, path: str = "index.json"):
        """Load index from disk"""
        if os.path.exists(path):
            with open(path, 'r', encoding='utf-8') as f:
                data = json.load(f)
                self.chunks = data['chunks']
                self.stats = data['stats']
                print(f"📂 Loaded {len(self.chunks)} chunks from {path}")
                return True
        return False

def main():
    """Demo the real indexer"""
    print("="*70)
    print("         REAL FILE INDEXER - Indexing Your Actual Code")
    print("="*70)
    
    # Create indexer
    indexer = RealFileIndexer()
    
    # Check if we should re-index
    if len(sys.argv) > 1 and sys.argv[1] == '--reindex':
        print("Force re-indexing...")
        indexer.index_directory()
        indexer.save_index()
    elif indexer.load_index():
        print("Using existing index. Run with --reindex to rebuild.")
    else:
        # Actually index the codebase
        indexer.index_directory()
        indexer.save_index()
    
    # Interactive search
    print("\n🔍 INTERACTIVE SEARCH (type 'quit' to exit)")
    print("Commands: 'stats' for statistics, 'help' for help")
    print("-"*70)
    
    try:
        while True:
            query = input("\nSEARCH> ").strip()
            
            if query.lower() in ['quit', 'exit', 'q']:
                break
                
            if query.lower() == 'stats':
                print(f"\n📊 Index Statistics:")
                print(f"   Total files: {indexer.stats['total_files']}")
                print(f"   Total chunks: {indexer.stats['total_chunks']}")
                print(f"   Total size: {indexer.stats['total_bytes'] / 1024 / 1024:.2f} MB")
                print(f"   File types: {dict(indexer.stats['file_types'])}")
                continue
                
            if query.lower() == 'help':
                print("\n📖 Search Tips:")
                print("   - Function names: 'create_sphere', 'boolean_union'")
                print("   - Types: 'BRepModel', 'SessionManager'")
                print("   - Any code: 'websocket', 'NURBS', 'timeline'")
                print("   - Commands: 'stats', 'help', 'quit'")
                continue
                
            if not query:
                continue
                
            # Search the actual indexed content
            start_time = time.time()
            results = indexer.search(query, limit=10)
            search_time = (time.time() - start_time) * 1000
            
            if results:
                print(f"\n✅ Found {len(results)} results in {search_time:.1f}ms:")
                for i, result in enumerate(results[:10], 1):
                    chunk = result['chunk']
                    print(f"\n{i}. Score: {result['score']:.1f} [{', '.join(result['match_types']).upper()}]")
                    print(f"   📄 {chunk['file']} (lines {chunk['start_line']}-{chunk['end_line']})")
                    if chunk['symbols']:
                        print(f"   🔤 {', '.join(chunk['symbols'][:5])}")
                    # Show first 2 lines of content
                    lines = chunk['content'].split('\n')[:2]
                    for line in lines:
                        if line.strip():
                            print(f"      {line[:80]}...")
            else:
                print(f"❌ No results found for '{query}'")
                
    except (KeyboardInterrupt, EOFError):
        pass
            
    print("\n👋 Goodbye!")

if __name__ == "__main__":
    main()