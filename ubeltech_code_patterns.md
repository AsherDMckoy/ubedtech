# University of Belize Frontend — Code Patterns & Template Structure

## Table of Contents
1. Askama Template Structure
2. Alpine.js Patterns
3. CSS Architecture (Variables & Responsive)
4. Cache Utility Module
5. Component Examples

---

## 1. Askama Template Structure

### Project Layout
```
backend/
├── src/
│   ├── main.rs
│   └── handlers/
├── templates/
│   ├── base.html          # Shared layout, rail nav
│   ├── partials/
│   │   ├── rail_nav.html  # Rail navigation component
│   │   ├── modal.html     # Reusable modal dialog
│   │   ├── schedule_weekly.html
│   │   ├── schedule_calendar.html
│   │   ├── campus_events_aside.html
│   │   └── cards/
│   │       ├── class_card.html
│   │       ├── gpa_card.html
│   │       └── registration_card.html
│   └── pages/
│       ├── dashboard.html
│       ├── grades.html
│       ├── schedule.html
│       ├── history.html
│       └── catalog.html
└── static/
    ├── css/
    │   ├── base.css        # Global styles, CSS vars
    │   ├── rail.css        # Rail nav animations
    │   ├── schedule.css    # Schedule page layout
    │   └── cache.css       # Loading/refresh UI
    └── js/
        ├── cache.js        # Caching utility (ES6 module)
        ├── alpine-config.js # Alpine setup & stores
        └── utils.js        # Shared helpers
```

### Base Layout (Askama Template)

**`templates/base.html`**
```html
<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>{% block title %}Dashboard{% endblock %} - University of Belize</title>
    
    <link rel="stylesheet" href="/static/css/base.css">
    <link rel="stylesheet" href="/static/css/rail.css">
    <link rel="stylesheet" href="/static/css/schedule.css">
    
    <!-- Alpine.js from CDN (CSP-safe, no eval) -->
    <script defer src="https://cdn.jsdelivr.net/npm/alpinejs@3.x.x/dist/cdn.min.js"></script>
    
    <style>
        :root {
            --color-primary-purple: #6366f1;      /* Brand/structure */
            --color-gold: #fbbf24;                /* Acting color */
            --color-dark-purple: #312e81;         /* Dark mode tint */
            --color-near-black: #0f0f0f;          /* Dark mode bg */
            --color-light-gray: #f3f4f6;
            --color-border: #e5e7eb;
            
            --rail-width-collapsed: 48px;
            --rail-width-expanded: 240px;
            --rail-animation-duration: 300ms;
            --rail-animation-curve: cubic-bezier(0.4, 0, 0.2, 1);
            
            --font-serif: "Cambria", serif;
            --font-sans: "Calibri", sans-serif;
            
            --spacing-xs: 4px;
            --spacing-sm: 8px;
            --spacing-md: 16px;
            --spacing-lg: 24px;
            --spacing-xl: 32px;
        }
    </style>
</head>
<body x-data="appState()">
    <div class="app-container">
        <!-- Rail Navigation -->
        {% include "partials/rail_nav.html" %}
        
        <!-- Main Content -->
        <main class="main-content">
            {% block header %}
            <header class="page-header">
                <h1>{% block page_title %}Dashboard{% endblock %}</h1>
            </header>
            {% endblock %}
            
            {% block content %}{% endblock %}
        </main>
    </div>
    
    <!-- Logout Modal (shared across all pages) -->
    {% include "partials/modal.html" %}
    
    <!-- Alpine config & utilities -->
    <script type="module">
        import { Cache } from '/static/js/cache.js';
        import { appState } from '/static/js/alpine-config.js';
        
        // Expose to window for Alpine
        window.Cache = Cache;
        window.appState = appState;
    </script>
</body>
</html>
```

### Rail Navigation Component

**`templates/partials/rail_nav.html`**
```html
<aside class="rail-nav" 
       x-data="railNav()" 
       :class="{ 'rail-nav--expanded': isExpanded }"
       @mouseenter="expand()" 
       @mouseleave="collapse()">
    
    <nav class="rail-nav__content">
        <div class="rail-nav__header">
            <button class="rail-nav__toggle" @click="toggle()" aria-label="Toggle navigation">
                <svg class="rail-nav__icon" viewBox="0 0 24 24" fill="currentColor">
                    <path d="M3 18h18v-2H3v2zm0-5h18v-2H3v2zm0-7v2h18V6H3z"/>
                </svg>
                <span class="rail-nav__text">Menu</span>
            </button>
        </div>
        
        <ul class="rail-nav__list">
            {% for item in nav_items %}
            <li class="rail-nav__item" :class="{ 'rail-nav__item--active': isActive('{{ item.route }}') }">
                <a href="{{ item.route }}" class="rail-nav__link" :title="!isExpanded ? '{{ item.label }}' : ''">
                    <svg class="rail-nav__icon" viewBox="0 0 24 24" fill="currentColor">
                        {# SVG path varies per item #}
                        {% if item.icon == "dashboard" %}
                        <path d="M3 13h8V3H3v10zm0 8h8v-6H3v6zm10 0h8V11h-8v10zm0-18v6h8V3h-8z"/>
                        {% endif %}
                        {# ... other icons ... #}
                    </svg>
                    <span class="rail-nav__text">{{ item.label }}</span>
                </a>
            </li>
            {% endfor %}
        </ul>
        
        <!-- Role-specific items (instructor vs student) -->
        {% if user.role == "instructor" %}
        <div class="rail-nav__section">
            <h3 class="rail-nav__section-title">
                <span class="rail-nav__text">Instructor</span>
            </h3>
            <ul class="rail-nav__list">
                <li class="rail-nav__item">
                    <a href="/my-courses" class="rail-nav__link" title="My Courses">
                        <svg class="rail-nav__icon" viewBox="0 0 24 24" fill="currentColor">
                            <path d="M4 6h16v2H4zm0 5h16v2H4zm0 5h16v2H4z"/>
                        </svg>
                        <span class="rail-nav__text">My Courses</span>
                        <span class="rail-nav__hint">📋 Roster & attendance</span>
                    </a>
                </li>
                <li class="rail-nav__item">
                    <a href="/grade-management" class="rail-nav__link" title="Grade Management">
                        <svg class="rail-nav__icon" viewBox="0 0 24 24" fill="currentColor">
                            <path d="M19 3H5c-1.1 0-2 .9-2 2v14c0 1.1.9 2 2 2h14c1.1 0 2-.9 2-2V5c0-1.1-.9-2-2-2zm0 16H5V5h14v14zm-5.04-6.71l-2.75 3.54-2.16-2.66c-.44-.53-1.25-.53-1.69 0-.44.54-.44 1.39 0 1.93l3 3.67c.44.53 1.25.53 1.69 0l4-5.07c.44-.54.44-1.39 0-1.93-.45-.54-1.25-.54-1.69 0z"/>
                        </svg>
                        <span class="rail-nav__text">Grade Management</span>
                        <span class="rail-nav__hint">📊 Bulk entry & export</span>
                    </a>
                </li>
            </ul>
        </div>
        {% endif %}
    </nav>
    
    <div class="rail-nav__footer">
        <button class="rail-nav__logout" @click="openLogoutModal()" title="Logout">
            <svg class="rail-nav__icon" viewBox="0 0 24 24" fill="currentColor">
                <path d="M17 7l-1.41 1.41L18.17 11H8v2h10.17l-2.58 2.58L17 17l5-5zM4 5h8V3H4c-1.1 0-2 .9-2 2v14c0 1.1.9 2 2 2h8v-2H4V5z"/>
            </svg>
            <span class="rail-nav__text">Logout</span>
        </button>
    </div>
</aside>

<script type="module">
function railNav() {
    return {
        isExpanded: false,
        
        expand() {
            this.isExpanded = true;
        },
        
        collapse() {
            this.isExpanded = false;
        },
        
        toggle() {
            this.isExpanded = !this.isExpanded;
        },
        
        isActive(route) {
            return window.location.pathname === route;
        },
        
        openLogoutModal() {
            this.$dispatch('open-logout-modal');
        }
    }
}
</script>
```

### Logout Modal

**`templates/partials/modal.html`**
```html
<div class="modal-backdrop" 
     x-show="showLogoutModal" 
     @click="showLogoutModal = false"
     style="display: none;">
    
    <div class="modal" @click.stop="" x-transition>
        <div class="modal__header">
            <h2 class="modal__title">Confirm Logout</h2>
        </div>
        
        <div class="modal__body">
            <p>Are you sure you want to log out? You will need to log in again to access your account.</p>
        </div>
        
        <div class="modal__footer">
            <button class="btn btn--secondary" @click="showLogoutModal = false">
                Cancel
            </button>
            <form method="POST" action="/logout" style="display: inline;">
                <button type="submit" class="btn btn--danger">
                    Confirm Logout
                </button>
            </form>
        </div>
    </div>
</div>

<script type="module">
// Alpine will catch the 'open-logout-modal' dispatch
document.addEventListener('alpine:init', () => {
    Alpine.store('modal', {
        open: false,
        message: 'Are you sure?'
    });
});
</script>
```

---

## 2. Alpine.js Patterns

### Alpine Config & App State

**`static/js/alpine-config.js`**
```javascript
export function appState() {
    return {
        // Auth & User
        userRole: null, // 'student' or 'instructor'
        userName: '',
        
        // Modal state
        showLogoutModal: false,
        
        // Navigation
        currentPage: '',
        
        // Cache
        cache: null,
        lastUpdated: {},
        
        // Calendar & Schedule
        calendarData: {},
        selectedDate: null,
        
        // Catalog search
        searchQuery: '',
        searchResults: [],
        searchLoading: false,
        
        // Initialize on page load
        async init() {
            console.log('Initializing app state...');
            this.cache = new window.Cache();
            
            // Load user from meta tag or API
            this.userRole = document.querySelector('meta[name="user-role"]')?.content || 'student';
            this.userName = document.querySelector('meta[name="user-name"]')?.content || '';
            
            // Pre-load calendar data if on dashboard/schedule
            if (this.shouldLoadCalendar()) {
                await this.loadCalendarData();
            }
            
            // Listen for logout modal trigger
            document.addEventListener('open-logout-modal', () => {
                this.showLogoutModal = true;
            });
        },
        
        shouldLoadCalendar() {
            const path = window.location.pathname;
            return path === '/dashboard' || path === '/schedule';
        },
        
        async loadCalendarData() {
            const cached = await this.cache.get('calendarData');
            if (cached) {
                this.calendarData = cached;
                this.lastUpdated.calendar = new Date(this.cache.getMetadata('calendarData')?.timestamp);
                return;
            }
            
            try {
                const response = await fetch('/api/schedule/calendar');
                const data = await response.json();
                this.calendarData = data;
                await this.cache.set('calendarData', data, { ttl: 7 * 24 * 60 * 60 }); // 7 days
                this.lastUpdated.calendar = new Date();
            } catch (err) {
                console.error('Failed to load calendar data:', err);
            }
        },
        
        hasEventsOnDate(dateString) {
            return this.calendarData[dateString]?.length > 0;
        },
        
        getEventsForDate(dateString) {
            return this.calendarData[dateString] || [];
        },
        
        async searchCatalog(query) {
            if (!query.trim()) {
                this.searchResults = [];
                return;
            }
            
            this.searchLoading = true;
            try {
                const response = await fetch(`/api/catalog/search?q=${encodeURIComponent(query)}`);
                const data = await response.json();
                this.searchResults = data.results || [];
            } catch (err) {
                console.error('Search failed:', err);
                this.searchResults = [];
            } finally {
                this.searchLoading = false;
            }
        },
        
        // Debounced search (300ms)
        debouncedSearch: null,
        onSearchInput(query) {
            if (this.debouncedSearch) clearTimeout(this.debouncedSearch);
            this.debouncedSearch = setTimeout(() => {
                this.searchCatalog(query);
            }, 300);
        }
    };
}
```

### Component Examples

**Catalog Search Component (Template Fragment)**
```html
<div x-data="catalogSearch()" class="catalog-search">
    <input 
        type="text"
        x-model="query"
        @input="onSearchInput($event.target.value)"
        placeholder="Search courses..."
        class="search-input"
        aria-label="Search catalog">
    
    <div x-show="searchLoading" class="spinner"></div>
    
    <div x-show="query.trim() && !searchLoading" class="search-results">
        <ul>
            <template x-for="(course, index) in searchResults" :key="course.id">
                <li class="result-item" 
                    :class="{ 'result-item--selected': selectedIndex === index }"
                    @click="selectCourse(course)">
                    <div class="result-item__code">{{ course.code }}</div>
                    <div class="result-item__title">{{ course.title }}</div>
                    <div class="result-item__credits">{{ course.credits }} credits</div>
                </li>
            </template>
        </ul>
        <div x-show="searchResults.length === 0" class="search-empty">
            No courses found
        </div>
    </div>
</div>

<script type="module">
function catalogSearch() {
    return {
        query: '',
        searchResults: [],
        searchLoading: false,
        selectedIndex: -1,
        
        onSearchInput(query) {
            const app = Alpine.store('app');
            app.onSearchInput(query);
            this.query = query;
        },
        
        selectCourse(course) {
            // Handle course selection (add to cart, etc.)
            console.log('Selected:', course);
            this.query = '';
            this.searchResults = [];
        }
    }
}
</script>
```

**Calendar Hover Component (Template Fragment)**
```html
<div class="calendar-month">
    <template x-for="date in monthDays" :key="date.toISOString()">
        <div class="calendar-day"
             :class="{ 
                 'calendar-day--has-events': hasEventsOnDate(date),
                 'calendar-day--today': isToday(date),
                 'calendar-day--selected': isSelected(date)
             }"
             @mouseenter="highlightDate(date)"
             @mouseleave="clearHighlight()"
             @click="selectDate(date)">
            
            <div class="calendar-day__date">{{ date.getDate() }}</div>
            
            <div x-show="hasEventsOnDate(date)" class="calendar-day__badge">
                {{ getEventsForDate(date).length }}
            </div>
        </div>
    </template>
</div>

<script type="module">
function calendarMonth() {
    return {
        monthDays: [],
        highlightedDate: null,
        
        init() {
            // Generate days for current month
            const today = new Date();
            const firstDay = new Date(today.getFullYear(), today.getMonth(), 1);
            const lastDay = new Date(today.getFullYear(), today.getMonth() + 1, 0);
            
            this.monthDays = [];
            for (let d = new Date(firstDay); d <= lastDay; d.setDate(d.getDate() + 1)) {
                this.monthDays.push(new Date(d));
            }
        },
        
        hasEventsOnDate(date) {
            const app = Alpine.store('app');
            const dateStr = this.formatDate(date);
            return app.hasEventsOnDate(dateStr);
        },
        
        getEventsForDate(date) {
            const app = Alpine.store('app');
            const dateStr = this.formatDate(date);
            return app.getEventsForDate(dateStr);
        },
        
        formatDate(date) {
            return date.toISOString().split('T')[0];
        },
        
        isToday(date) {
            const today = new Date();
            return date.toDateString() === today.toDateString();
        },
        
        isSelected(date) {
            const app = Alpine.store('app');
            return app.selectedDate?.toDateString() === date.toDateString();
        },
        
        selectDate(date) {
            Alpine.store('app').selectedDate = date;
            this.showDayDetail(date);
        },
        
        highlightDate(date) {
            // Instant highlight (no animation, just state change)
            this.highlightedDate = date;
        },
        
        clearHighlight() {
            this.highlightedDate = null;
        },
        
        showDayDetail(date) {
            // Dispatch event or open modal with day's events
            Alpine.store('app').showDayDetail = true;
            Alpine.store('app').selectedDayEvents = this.getEventsForDate(date);
        }
    }
}
</script>
```

---

## 3. CSS Architecture

### Base Styles & Variables

**`static/css/base.css`**
```css
* {
    margin: 0;
    padding: 0;
    box-sizing: border-box;
}

:root {
    --color-primary-purple: #6366f1;
    --color-gold: #fbbf24;
    --color-dark-purple: #312e81;
    --color-near-black: #0f0f0f;
    --color-light-gray: #f3f4f6;
    --color-border: #e5e7eb;
    
    --spacing-xs: 4px;
    --spacing-sm: 8px;
    --spacing-md: 16px;
    --spacing-lg: 24px;
    --spacing-xl: 32px;
    
    --font-serif: "Cambria", serif;
    --font-sans: "Calibri", sans-serif;
}

body {
    font-family: var(--font-sans);
    background-color: #fff;
    color: #1f2937;
    line-height: 1.5;
}

.app-container {
    display: grid;
    grid-template-columns: var(--rail-width-collapsed) 1fr;
    min-height: 100vh;
}

.main-content {
    padding: var(--spacing-md) var(--spacing-md);
    overflow-y: auto;
    max-width: 1400px;
    margin: 0 auto;
    width: 100%;
}

/* Typography */
h1, h2, h3, h4, h5, h6 {
    font-family: var(--font-serif);
    font-weight: 600;
}

/* Buttons */
.btn {
    padding: var(--spacing-sm) var(--spacing-md);
    border: 1px solid transparent;
    border-radius: 4px;
    cursor: pointer;
    font-family: var(--font-sans);
    font-size: 14px;
    transition: background-color 200ms, color 200ms;
}

.btn--primary {
    background-color: var(--color-primary-purple);
    color: white;
}

.btn--primary:hover {
    background-color: var(--color-dark-purple);
}

.btn--secondary {
    background-color: var(--color-light-gray);
    color: #1f2937;
}

.btn--secondary:hover {
    background-color: #e5e7eb;
}

.btn--danger {
    background-color: #ef4444;
    color: white;
}

.btn--danger:hover {
    background-color: #dc2626;
}
```

### Rail Navigation Styles

**`static/css/rail.css`**
```css
.rail-nav {
    grid-column: 1;
    grid-row: 1 / -1;
    width: var(--rail-width-collapsed);
    background-color: var(--color-primary-purple);
    color: white;
    padding: var(--spacing-md) 0;
    display: flex;
    flex-direction: column;
    gap: var(--spacing-lg);
    transition: width var(--rail-animation-duration) var(--rail-animation-curve);
    position: relative;
}

.rail-nav--expanded {
    width: var(--rail-width-expanded);
}

.rail-nav__content {
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: var(--spacing-lg);
    overflow-y: auto;
}

.rail-nav__header {
    padding: 0 var(--spacing-sm);
}

.rail-nav__toggle {
    width: 100%;
    background: none;
    border: none;
    color: white;
    cursor: pointer;
    display: flex;
    align-items: center;
    gap: var(--spacing-sm);
    padding: var(--spacing-sm);
    border-radius: 4px;
    transition: background-color 200ms;
}

.rail-nav__toggle:hover {
    background-color: rgba(255, 255, 255, 0.1);
}

.rail-nav__icon {
    width: 24px;
    height: 24px;
    flex-shrink: 0;
}

.rail-nav__text {
    opacity: 0;
    width: 0;
    overflow: hidden;
    white-space: nowrap;
    transition: opacity 200ms, width 200ms;
}

.rail-nav--expanded .rail-nav__text {
    opacity: 1;
    width: auto;
}

.rail-nav__list {
    list-style: none;
    display: flex;
    flex-direction: column;
    gap: var(--spacing-xs);
    padding: 0 var(--spacing-sm);
}

.rail-nav__item {
    position: relative;
}

.rail-nav__item--active .rail-nav__link {
    background-color: var(--color-gold);
    color: var(--color-dark-purple);
}

.rail-nav__item--active .rail-nav__link::before {
    content: '';
    position: absolute;
    left: 0;
    top: 0;
    bottom: 0;
    width: 3px;
    background-color: var(--color-gold);
}

.rail-nav__link {
    display: flex;
    align-items: center;
    gap: var(--spacing-sm);
    padding: var(--spacing-sm) var(--spacing-md);
    color: white;
    text-decoration: none;
    border-radius: 4px;
    transition: background-color 200ms;
    position: relative;
}

.rail-nav__link:hover {
    background-color: rgba(255, 255, 255, 0.1);
}

.rail-nav__hint {
    opacity: 0;
    font-size: 12px;
    width: 0;
    overflow: hidden;
    white-space: nowrap;
    transition: opacity 200ms;
}

.rail-nav--expanded .rail-nav__hint {
    opacity: 0.8;
    width: auto;
    display: block;
}

.rail-nav__section {
    padding: var(--spacing-md) var(--spacing-sm);
    border-top: 1px solid rgba(255, 255, 255, 0.1);
}

.rail-nav__section-title {
    font-size: 12px;
    font-weight: 600;
    text-transform: uppercase;
    opacity: 0.6;
    margin-bottom: var(--spacing-sm);
}

.rail-nav__footer {
    padding: 0 var(--spacing-sm) var(--spacing-md);
    border-top: 1px solid rgba(255, 255, 255, 0.1);
}

.rail-nav__logout {
    width: 100%;
    display: flex;
    align-items: center;
    gap: var(--spacing-sm);
    background: none;
    border: none;
    color: white;
    cursor: pointer;
    padding: var(--spacing-sm) var(--spacing-md);
    border-radius: 4px;
    transition: background-color 200ms;
}

.rail-nav__logout:hover {
    background-color: rgba(255, 255, 255, 0.1);
}

/* Responsive: hide rail on mobile, show bottom sheet instead */
@media (max-width: 768px) {
    .app-container {
        grid-template-columns: 1fr;
    }
    
    .rail-nav {
        position: fixed;
        bottom: 0;
        left: 0;
        right: 0;
        width: 100%;
        height: auto;
        flex-direction: row;
        border-top: 1px solid var(--color-border);
        justify-content: space-around;
        padding: var(--spacing-sm) 0;
    }
    
    .rail-nav__content {
        flex-direction: row;
        justify-content: space-around;
        width: 100%;
    }
    
    .main-content {
        padding-bottom: 80px; /* Space for bottom nav */
    }
}
```

### Schedule Styles

**`static/css/schedule.css`**
```css
.schedule-container {
    display: grid;
    grid-template-columns: 1fr 280px;
    gap: var(--spacing-lg);
}

.schedule-main {
    display: flex;
    flex-direction: column;
    gap: var(--spacing-lg);
}

/* Weekly Schedule */
.schedule-weekly {
    display: grid;
    grid-template-columns: repeat(6, 1fr);
    gap: var(--spacing-md);
}

.day-card {
    background: white;
    border: 1px solid var(--color-border);
    border-radius: 8px;
    padding: var(--spacing-md);
    min-height: 180px;
    display: flex;
    flex-direction: column;
    gap: var(--spacing-sm);
}

.day-card__date {
    font-weight: 600;
    color: var(--color-primary-purple);
    font-size: 14px;
}

.day-card__classes {
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: var(--spacing-sm);
}

.class-item {
    padding: var(--spacing-sm);
    background-color: var(--color-light-gray);
    border-left: 3px solid var(--color-gold);
    border-radius: 4px;
    font-size: 13px;
}

/* Full Month Calendar */
.calendar-month {
    display: grid;
    grid-template-columns: repeat(7, 1fr);
    gap: var(--spacing-xs);
    background: white;
    border: 1px solid var(--color-border);
    border-radius: 8px;
    padding: var(--spacing-md);
    min-height: 400px;
}

.calendar-day {
    aspect-ratio: 1;
    padding: var(--spacing-sm);
    border: 1px solid var(--color-border);
    border-radius: 4px;
    cursor: pointer;
    position: relative;
    transition: background-color 150ms, border-color 150ms;
}

.calendar-day:hover {
    border-color: var(--color-gold);
    background-color: rgba(251, 191, 36, 0.1);
}

.calendar-day--has-events {
    background-color: var(--color-gold);
    color: var(--color-dark-purple);
    border-color: var(--color-gold);
    font-weight: 600;
}

.calendar-day--has-events:hover {
    background-color: #f59e0b;
}

.calendar-day--today {
    border: 2px solid var(--color-gold);
}

.calendar-day--selected {
    background-color: var(--color-primary-purple);
    color: white;
}

.calendar-day__date {
    font-size: 14px;
    font-weight: 500;
}

.calendar-day__badge {
    position: absolute;
    top: 4px;
    right: 4px;
    background-color: rgba(0, 0, 0, 0.5);
    color: white;
    border-radius: 50%;
    width: 20px;
    height: 20px;
    display: flex;
    align-items: center;
    justify-content: center;
    font-size: 11px;
    font-weight: 600;
}

.calendar-day--has-events .calendar-day__badge {
    background-color: var(--color-dark-purple);
}

/* Campus Events Aside */
.campus-events-aside {
    background: white;
    border: 1px solid var(--color-border);
    border-radius: 8px;
    padding: var(--spacing-md);
    max-height: 600px;
    overflow-y: auto;
}

.campus-events-aside__title {
    font-family: var(--font-serif);
    font-size: 16px;
    font-weight: 600;
    margin-bottom: var(--spacing-md);
}

.event-item {
    padding: var(--spacing-md);
    border-bottom: 1px solid var(--color-border);
    font-size: 14px;
}

.event-item:last-child {
    border-bottom: none;
}

.event-item__date {
    color: var(--color-gold);
    font-weight: 600;
    font-size: 12px;
    text-transform: uppercase;
}

.event-item__name {
    color: #1f2937;
    margin-top: var(--spacing-xs);
}

/* Responsive */
@media (max-width: 1024px) {
    .schedule-container {
        grid-template-columns: 1fr;
    }
    
    .campus-events-aside {
        display: none;
    }
}

@media (max-width: 768px) {
    .schedule-weekly {
        grid-template-columns: 1fr 1fr;
    }
    
    .calendar-month {
        display: none;
    }
}
```

---

## 4. Cache Utility Module

**`static/js/cache.js`**
```javascript
export class Cache {
    constructor() {
        this.prefix = 'ubeltech_';
        this.storage = localStorage;
    }
    
    async get(key) {
        const fullKey = this.prefix + key;
        const item = this.storage.getItem(fullKey);
        
        if (!item) return null;
        
        try {
            const parsed = JSON.parse(item);
            
            // Check if expired
            if (parsed.ttl && Date.now() > parsed.expiresAt) {
                this.storage.removeItem(fullKey);
                return null;
            }
            
            return parsed.value;
        } catch (err) {
            console.error(`Cache parse error for ${key}:`, err);
            this.storage.removeItem(fullKey);
            return null;
        }
    }
    
    async set(key, value, options = {}) {
        const fullKey = this.prefix + key;
        const { ttl = 24 * 60 * 60 } = options; // Default 24 hours
        
        const item = {
            value,
            ttl,
            timestamp: Date.now(),
            expiresAt: Date.now() + (ttl * 1000)
        };
        
        try {
            this.storage.setItem(fullKey, JSON.stringify(item));
            return true;
        } catch (err) {
            console.error(`Cache set error for ${key}:`, err);
            return false;
        }
    }
    
    async delete(key) {
        const fullKey = this.prefix + key;
        this.storage.removeItem(fullKey);
    }
    
    async clear() {
        const keys = [];
        for (let i = 0; i < this.storage.length; i++) {
            const key = this.storage.key(i);
            if (key.startsWith(this.prefix)) {
                keys.push(key);
            }
        }
        keys.forEach(key => this.storage.removeItem(key));
    }
    
    getMetadata(key) {
        const fullKey = this.prefix + key;
        const item = this.storage.getItem(fullKey);
        if (!item) return null;
        
        try {
            const parsed = JSON.parse(item);
            return {
                timestamp: parsed.timestamp,
                expiresAt: parsed.expiresAt,
                ttl: parsed.ttl
            };
        } catch (err) {
            return null;
        }
    }
}

export function formatCacheAge(timestamp) {
    const now = Date.now();
    const diff = now - timestamp;
    const seconds = Math.floor(diff / 1000);
    const minutes = Math.floor(seconds / 60);
    const hours = Math.floor(minutes / 60);
    const days = Math.floor(hours / 24);
    
    if (days > 0) return `${days} day${days > 1 ? 's' : ''} ago`;
    if (hours > 0) return `${hours} hour${hours > 1 ? 's' : ''} ago`;
    if (minutes > 0) return `${minutes} minute${minutes > 1 ? 's' : ''} ago`;
    return 'just now';
}
```

---

## 5. Modal Component

**`templates/partials/modal.html`**
```html
<div class="modal-backdrop" 
     x-show="showLogoutModal" 
     @click="showLogoutModal = false"
     @keydown.escape="showLogoutModal = false"
     x-transition
     style="display: none;">
    
    <div class="modal" @click.stop="">
        <div class="modal__header">
            <h2 class="modal__title">Confirm Logout</h2>
        </div>
        
        <div class="modal__body">
            <p>Are you sure you want to log out?</p>
        </div>
        
        <div class="modal__footer">
            <button class="btn btn--secondary" @click="showLogoutModal = false">
                Cancel
            </button>
            <form method="POST" action="/logout" style="display: inline;">
                <button type="submit" class="btn btn--danger">
                    Confirm Logout
                </button>
            </form>
        </div>
    </div>
</div>

<style>
.modal-backdrop {
    position: fixed;
    top: 0;
    left: 0;
    right: 0;
    bottom: 0;
    background-color: rgba(0, 0, 0, 0.5);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 1000;
}

.modal {
    background: white;
    border-radius: 8px;
    box-shadow: 0 20px 25px -5px rgba(0, 0, 0, 0.1);
    min-width: 300px;
    max-width: 400px;
    animation: slideIn 300ms var(--rail-animation-curve);
}

@keyframes slideIn {
    from {
        opacity: 0;
        transform: scale(0.95);
    }
    to {
        opacity: 1;
        transform: scale(1);
    }
}

.modal__header {
    padding: var(--spacing-lg) var(--spacing-lg) var(--spacing-md);
    border-bottom: 1px solid var(--color-border);
}

.modal__title {
    margin: 0;
    font-size: 18px;
    font-weight: 600;
}

.modal__body {
    padding: var(--spacing-lg);
}

.modal__body p {
    margin: 0;
    color: #666;
}

.modal__footer {
    padding: var(--spacing-md) var(--spacing-lg);
    border-top: 1px solid var(--color-border);
    display: flex;
    gap: var(--spacing-md);
    justify-content: flex-end;
}
</style>
```

---

## Key Architectural Decisions

1. **No optimistic state**: Modal and logout wait for server response
2. **3 animated surfaces max**: Rail expand, search results fade, modal slide
3. **Pre-loaded calendar data**: Single API call on dashboard, cached locally
4. **Client-side hover**: Zero lag on calendar date highlighting
5. **localStorage caching**: TTL-based invalidation, user-scoped keys
6. **Alpine.js data flow**: Single `appState()` store, components read from it
7. **Responsive breakpoints**: Desktop (1024px+), Tablet (768-1024px), Mobile (<768px)
8. **Role-based navigation**: Instructor vs Student menu items generated server-side

---

## Next Steps for Claude Code

1. Start with Rail Navigation (Prompt 1)
2. Implement caching infrastructure (Prompt 5)
3. Refactor calendar & hover interactions (Prompt 2)
4. Redesign schedule page (Prompt 6)
5. Add instant search (Prompt 3)
6. Implement logout modal (Prompt 4)
7. Expand main content layout (Prompt 7)
8. Add role-based navigation (Prompt 8)
9. Implement page-specific caching (Prompt 9)
