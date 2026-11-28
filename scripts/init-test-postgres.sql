-- Disposable test databases copy template1.
\connect template1
CREATE EXTENSION IF NOT EXISTS vector;

\connect postgres
CREATE ROLE yorishiro LOGIN PASSWORD 'yorishiro' NOSUPERUSER NOBYPASSRLS CREATEDB CREATEROLE;
\if :{?test_database}
\else
\set test_database yorishiro
\endif
CREATE DATABASE :"test_database" OWNER yorishiro;
