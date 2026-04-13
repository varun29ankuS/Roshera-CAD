#!/usr/bin/env python3
"""
TurboRAG Interactive Search Client
Real-time search interface for your indexed codebase
"""

import asyncio
import aiohttp
import json
import time
from pathlib import Path
from typing import List, Dict, Optional
from datetime import datetime
import sys

class TurboRAGClient:
    def __init__(self, base_url: str = "http://localhost:3030"):
        self.base_url = base_url
        self.session = None
        self.stats = {}
        
    async def __aenter__(self):
        self.session = aiohttp.ClientSession()
        await self.check_health()
        return self
        
    async def __aexit__(self, exc_type, exc_val, exc_tb):
        if self.session:
            await self.session.close()
            
    async def check_health(self):
        """Check if TurboRAG server is running"""
        try:
            async with self.session.get(f"{self.base_url}/health") as resp:
                if resp.status == 200:
                    data = await resp.json()
                    print(f"✅ Connected to TurboRAG (status: {data.get('status', 'unknown')})")
                    return True
        except Exception as e:
            print(f"⚠️  TurboRAG server not responding at {self.base_url}")
            print(f"   Error: {e}")
            print("\n   Start the server with: cargo run --release")
            return False
            
    async def search(self, query: str, search_type: str = "hybrid", limit: int = 10) -> Dict:
        """Execute search query"""
        try:
            payload = {
                "query": query,
                "limit": limit,
                "search_type": search_type,
                "include_embeddings": False
            }
            
            async with self.session.post(
                f"{self.base_url}/api/search",
                json=payload
            ) as resp:
                if resp.status == 200:
                    return await resp.json()
                else:
                    return {"error": f"Search failed: {resp.status}"}
        except Exception as e:
            return {"error": str(e)}
            
    async def get_stats(self) -> Dict:
        """Get indexing statistics"""
        try:
            async with self.session.get(f"{self.base_url}/api/stats") as resp:
                if resp.status == 200:
                    return await resp.json()
        except:
            return {}
            
    async def get_metrics(self) -> Dict:
        """Get system metrics"""
        try:
            async with self.session.get(f"{self.base_url}/api/metrics") as resp:
                if resp.status == 200:
                    return await resp.json()
        except:
            return {}

class InteractiveSearch:
    def __init__(self):
        self.client = None
        self.history = []
        self.search_modes = {
            "1": ("hybrid", "🔄 Hybrid (BM25 + Vector)"),
            "2": ("vector", "🎯 Vector Semantic"),
            "3": ("bm25", "📝 BM25 Keyword"),
            "4": ("symbol", "🔤 Symbol Search"),
            "5": ("fuzzy", "✨ Fuzzy Match")
        }
        self.current_mode = "1"
        
    async def start(self):
        """Start interactive search session"""
        print("="*70)
        print("           TURBORAG - INTERACTIVE SEARCH")
        print("         Real-time Search in Your Codebase")
        print("="*70)
        
        async with TurboRAGClient() as client:
            self.client = client
            
            # Show initial stats
            await self.show_stats()
            
            # Show search modes
            self.show_modes()
            
            # Main search loop
            await self.search_loop()
            
    async def show_stats(self):
        """Display indexing statistics"""
        stats = await self.client.get_stats()
        metrics = await self.client.get_metrics()
        
        if stats:
            print("\n📊 INDEXING STATUS:")
            print("-"*40)
            print(f"Total Files: {stats.get('total_files', 0):,}")
            print(f"Total Chunks: {stats.get('total_chunks', 0):,}")
            print(f"Index Size: {stats.get('index_size_mb', 0):.1f} MB")
            
            if 'file_types' in stats:
                print("\nFile Types:")
                for ft, count in stats['file_types'].items():
                    print(f"  • {ft}: {count}")
                    
        if metrics and 'current_metrics' in metrics:
            m = metrics['current_metrics']
            print(f"\n⚡ Performance:")
            print(f"  • QPS: {m.get('qps', 0):.1f}")
            print(f"  • Avg Latency: {m.get('avg_latency_ms', 0):.1f}ms")
            print(f"  • Cache Hit Rate: {m.get('cache_hit_rate', 0)*100:.1f}%")
            
    def show_modes(self):
        """Display search modes"""
        print("\n🔍 SEARCH MODES:")
        print("-"*40)
        for key, (mode, desc) in self.search_modes.items():
            selected = "►" if key == self.current_mode else " "
            print(f" {selected} [{key}] {desc}")
            
    async def search_loop(self):
        """Main search interaction loop"""
        print("\n" + "="*70)
        print("Commands: [1-5] change mode | 'stats' | 'help' | 'quit'")
        print("="*70)
        
        while True:
            try:
                # Show prompt with current mode
                mode_name = self.search_modes[self.current_mode][1].split()[1]
                prompt = f"\n{mode_name}> "
                
                # Get user input
                query = input(prompt).strip()
                
                if not query:
                    continue
                    
                # Handle commands
                if query.lower() in ['quit', 'exit', 'q']:
                    print("\n👋 Goodbye!")
                    break
                    
                elif query in self.search_modes:
                    self.current_mode = query
                    print(f"✅ Switched to {self.search_modes[query][1]}")
                    continue
                    
                elif query.lower() == 'stats':
                    await self.show_stats()
                    continue
                    
                elif query.lower() == 'help':
                    self.show_help()
                    continue
                    
                elif query.lower() == 'history':
                    self.show_history()
                    continue
                    
                # Perform search
                await self.perform_search(query)
                
            except KeyboardInterrupt:
                print("\n\n👋 Search interrupted. Goodbye!")
                break
            except Exception as e:
                print(f"\n❌ Error: {e}")
                
    async def perform_search(self, query: str):
        """Execute and display search results"""
        # Add to history
        self.history.append((datetime.now(), query))
        
        # Start timer
        start_time = time.time()
        
        # Get search mode
        mode, _ = self.search_modes[self.current_mode]
        
        # Perform search
        print(f"\n🔎 Searching for: '{query}'...")
        result = await self.client.search(query, mode, limit=10)
        
        # Calculate time
        search_time = (time.time() - start_time) * 1000
        
        if 'error' in result:
            print(f"❌ Search failed: {result['error']}")
            return
            
        # Display results
        chunks = result.get('results', [])
        
        if not chunks:
            print(f"\n📭 No results found for '{query}'")
            print("   Try different terms or change search mode")
        else:
            print(f"\n📚 Found {len(chunks)} results in {search_time:.1f}ms:")
            print("-"*70)
            
            for i, chunk in enumerate(chunks, 1):
                self.display_result(i, chunk)
                
                if i < len(chunks):
                    print("-"*40)
                    
            # Show search statistics
            if 'stats' in result:
                stats = result['stats']
                print(f"\n📈 Search Statistics:")
                print(f"   • Documents scanned: {stats.get('docs_scanned', 0)}")
                print(f"   • Vectors compared: {stats.get('vectors_compared', 0)}")
                print(f"   • Cache hits: {stats.get('cache_hits', 0)}")
                
    def display_result(self, index: int, chunk: Dict):
        """Display a single search result"""
        score = chunk.get('score', 0.0)
        metadata = chunk.get('metadata', {})
        content = chunk.get('content', '')
        
        # Header
        print(f"\n{index}. Score: {score:.3f}")
        
        # File info
        file_path = metadata.get('file_path', 'unknown')
        file_type = metadata.get('file_type', 'unknown')
        lines = f"{metadata.get('start_line', 0)}-{metadata.get('end_line', 0)}"
        
        print(f"   📄 {file_path}")
        print(f"   📍 Lines: {lines} | Type: {file_type}")
        
        # Symbols if present
        symbols = metadata.get('symbols', [])
        if symbols:
            print(f"   🔤 Symbols: {', '.join(symbols[:5])}")
            
        # Content preview
        print(f"   📝 Content:")
        content_lines = content.split('\n')[:4]
        for line in content_lines:
            if line.strip():
                print(f"      {line[:80]}...")
                
    def show_help(self):
        """Display help information"""
        print("\n📖 HELP:")
        print("-"*40)
        print("Search Tips:")
        print("  • Function names: 'create_sphere', 'boolean_union'")
        print("  • Concepts: 'NURBS curve', 'WebSocket handler'")
        print("  • Symbols: 'BRepModel', 'SessionManager'")
        print("  • File types: 'primitive', 'export', 'api'")
        print("\nCommands:")
        print("  • [1-5]: Switch search mode")
        print("  • stats: Show indexing statistics")
        print("  • history: Show search history")
        print("  • help: Show this help")
        print("  • quit: Exit the program")
        
    def show_history(self):
        """Display search history"""
        if not self.history:
            print("\n📜 No search history yet")
            return
            
        print("\n📜 SEARCH HISTORY:")
        print("-"*40)
        for timestamp, query in self.history[-10:]:
            print(f"  {timestamp.strftime('%H:%M:%S')} - {query}")

async def main():
    """Main entry point"""
    # Check if server URL is provided
    if len(sys.argv) > 1:
        base_url = sys.argv[1]
        client = InteractiveSearch()
        client.client = TurboRAGClient(base_url)
    else:
        client = InteractiveSearch()
        
    await client.start()

if __name__ == "__main__":
    # For Windows color support
    import platform
    if platform.system() == 'Windows':
        import os
        os.system('color')
        
    try:
        asyncio.run(main())
    except KeyboardInterrupt:
        print("\n\n👋 Goodbye!")